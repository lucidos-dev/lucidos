pub mod client;
pub mod types;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::core::{McpServer, McpServerStore};
use crate::engine::context::{estimate_tokens_from_chars, tool_definitions_chars};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{
    ActorMode, EventMeta, McpEnablementSource, MessageOrigin, ThreadEvent,
};
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
    /// Whether this server's process is known to be gone.
    ///
    /// A client that will not lock has a call in flight, which is a server
    /// answering, so the probe is skipped rather than waited on. Both that and
    /// a failed `try_wait` answer "still running": unknown is never a "no"
    /// here, because the entry is what a live call dispatches against.
    fn has_exited(&self) -> bool {
        self.client
            .try_lock()
            .is_ok_and(|mut client| client.has_exited())
    }

    fn target(&self) -> CallTarget {
        CallTarget {
            client: Arc::clone(&self.client),
            tools: Arc::clone(&self.tools),
            server_name: self.server_config.name.clone(),
            auto_approve: self.server_config.auto_approve,
        }
    }
}

/// What a connected server's tools are good for, told to the model that just
/// connected it.
///
/// The array used to be frozen for the turn. The agent learned that by
/// failing, and started telling users to ask again in a new message. It now
/// refreshes per round, and the model has no way to observe that except by
/// being told. Pinned by
/// `every_way_a_server_comes_up_tells_the_model_its_tools_are_live_now`.
pub const CALLABLE_NOW: &str = "Its tools are callable right now, in this same turn: \
     use them instead of asking the user to send another message.";

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
    /// Moves whenever [`McpManager::tool_surface`] would answer differently.
    ///
    /// A turn reads the tool array once and then runs for many rounds. Without
    /// this, a server the agent started mid-turn stayed uncallable until the
    /// next user message. The counter is what lets a running turn ask "has the
    /// surface moved since I looked" for the price of one atomic load.
    ///
    /// A reader takes its stamp BEFORE it reads the map and the thread's
    /// enabled set, so a stamp is never newer than what it describes. A change
    /// to either bumps it after the change lands.
    tool_surface_generation: Arc<AtomicU64>,
    /// One lock per server id, held from the "is it running" check to the
    /// insert. Every turn setup starts its thread's servers, so without it two
    /// threads waking together spawn two processes for one server.
    start_locks: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Held from a disabled-tools write to its copy onto the running snapshot.
    /// Without it two writes could commit in one order and copy in the other,
    /// and dispatch would then disagree with the stored set.
    disabled_tools_writes: Mutex<()>,
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
/// `chars` and `tokens` are what the tools not switched off cost a thread that
/// enables the server, whether or not it is up: for a stopped one that answers
/// "what would this cost if a thread enabled it". [`McpCostTotals`] splits the
/// running servers from the stopped ones.
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

/// What the registered MCP servers cost a thread that enables them, split by
/// whether the process is running (ADR 0430).
///
/// Every token figure is [`estimate_tokens_from_chars`] of the matching char
/// figure, never a sum of per-tool tokens: the ratio is integer division, so
/// summing rounded parts drifts from the whole.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct McpCostTotals {
    pub servers: usize,
    pub running_servers: usize,
    /// What the running servers offer a thread that enables them.
    pub tools: usize,
    pub chars: usize,
    pub tokens: usize,
    /// What the stopped servers would add to a thread that enabled them.
    /// Excludes servers whose id cannot dispatch, since those can never run.
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

/// What the running servers one thread enabled contribute to its request, and
/// when that was true.
///
/// The two travel together because a turn holds them for many rounds and has
/// to decide whether what it holds is still current. See
/// [`McpManager::tool_surface`].
pub struct McpToolSurface {
    pub tools: Vec<ToolDefinition>,
    pub generation: u64,
    /// What did not fit under the ceiling, per server. Empty when all fit.
    pub dropped: Vec<McpDroppedTools>,
    /// The running servers the thread enabled, so the ones this slice offers
    /// tools for, plus any the ceiling cut to nothing.
    pub servers: BTreeSet<String>,
}

impl McpToolSurface {
    /// The one line that tells the model some tools were not sent, so it can
    /// tell the user. `None` when nothing was dropped.
    pub fn dropped_notice(&self) -> Option<String> {
        dropped_notice(&self.dropped)
    }
}

/// [`McpToolSurface::dropped_notice`] for any fit's leftovers.
pub fn dropped_notice(dropped: &[McpDroppedTools]) -> Option<String> {
    (!dropped.is_empty()).then(|| {
        format!(
            "[MCP TOOLS NOT SENT] The servers enabled in this thread offer more tools \
             than fit their share of this model's context, so these were left out: {}. \
             Tell the user, and suggest switching tools off in Settings or disabling a \
             server in this thread.",
            describe_dropped(dropped)
        )
    })
}

/// The tools of one server that did not fit under the ceiling, by wire name,
/// in the server's own listed order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct McpDroppedTools {
    pub server_id: String,
    pub tools: Vec<String>,
}

/// How many dropped names one server lists before it says "and N more".
const DROPPED_NAMES_SHOWN: usize = 10;

/// `server 'github' (3: a, b, c)`, joined with `; `.
fn describe_dropped(dropped: &[McpDroppedTools]) -> String {
    dropped
        .iter()
        .map(|d| {
            let shown: Vec<&str> = d
                .tools
                .iter()
                .take(DROPPED_NAMES_SHOWN)
                .map(String::as_str)
                .collect();
            let more = d.tools.len().saturating_sub(DROPPED_NAMES_SHOWN);
            let tail = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            format!(
                "server '{}' ({}: {}{})",
                d.server_id,
                d.tools.len(),
                shown.join(", "),
                tail
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The share of a request's char budget the MCP tools may take, as a divisor.
///
/// A `tools/list` is third-party input of any size, so the MCP tools must
/// never take more than this share. A quarter leaves the system prompt, the
/// engine's own tools and the conversation the rest.
const MCP_BUDGET_DIVISOR: usize = 4;

/// Chars the tools of the servers one thread enabled may take together, for a
/// model with this context window. Shared by the request path and the cost
/// report.
pub fn mcp_tool_char_ceiling(context_window: usize) -> usize {
    crate::engine::context::agent_context_char_budget(context_window) / MCP_BUDGET_DIVISOR
}

/// From this share of the MCP tool ceiling, the cost report warns before any tool
/// is left out.
const MCP_WARNING_PERCENT: usize = 80;

/// Every server enabled in one thread against the request budget, as the cost
/// report states it (ADR 0430). Chars throughout, the unit the request packer
/// budgets in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct McpBudget {
    /// What a whole request may carry: system prompt, tools and messages.
    pub request_chars: usize,
    /// The part of it the tools of one thread's servers may take together.
    pub mcp_ceiling_chars: usize,
    /// What that thread's request carries once the ceiling applies. Less than
    /// the offered figures exactly when something is dropped.
    pub sent_tools: usize,
    pub sent_chars: usize,
    pub sent_tokens: usize,
    /// What that thread loses to the ceiling.
    pub dropped: Vec<McpDroppedTools>,
    /// One line for the page once the tools are past or near the ceiling.
    pub warning: Option<String>,
}

impl McpBudget {
    fn new(context_window: usize, sent: &[ToolDefinition], dropped: Vec<McpDroppedTools>) -> Self {
        let sent_chars = tool_definitions_chars(sent);
        let mcp_ceiling_chars = mcp_tool_char_ceiling(context_window);
        let left_out: usize = dropped.iter().map(|d| d.tools.len()).sum();
        let warning = if left_out > 0 {
            Some(format!(
                "A thread that enables every server leaves {left_out} {} out, because \
                 together they offer more than the MCP share of the context: {}. Switch \
                 tools off, or enable fewer servers in one thread.",
                if left_out == 1 { "tool" } else { "tools" },
                describe_dropped(&dropped)
            ))
        } else if sent_chars.saturating_mul(100)
            >= mcp_ceiling_chars.saturating_mul(MCP_WARNING_PERCENT)
            && sent_chars > 0
        {
            Some(format!(
                "With every server enabled in one thread, MCP tools fill {}% of their \
                 share of the context. Past it, tools are left out of that thread's \
                 requests.",
                sent_chars.saturating_mul(100) / mcp_ceiling_chars.max(1)
            ))
        } else {
            None
        };
        Self {
            request_chars: crate::engine::context::agent_context_char_budget(context_window),
            mcp_ceiling_chars,
            sent_tools: sent.len(),
            sent_chars,
            sent_tokens: estimate_tokens_from_chars(sent_chars),
            dropped,
            warning,
        }
    }
}

/// Fit each server's offered tools under `ceiling`, deterministically.
///
/// Servers are taken in id order. Each gets a max-min fair share: a server
/// asking for less than an even split keeps all of it, and what it leaves
/// goes to the rest. Within its share a server keeps its tools in its own
/// listed order, skipping any that no longer fit.
fn fit_to_ceiling(
    mut servers: Vec<(String, Vec<ToolDefinition>)>,
    ceiling: usize,
) -> (Vec<ToolDefinition>, Vec<McpDroppedTools>) {
    servers.sort_by(|a, b| a.0.cmp(&b.0));
    let costs: Vec<Vec<usize>> = servers
        .iter()
        .map(|(_, defs)| {
            defs.iter()
                .map(|d| tool_definitions_chars(std::slice::from_ref(d)))
                .collect()
        })
        .collect();

    let mut by_demand: Vec<usize> = (0..servers.len()).collect();
    by_demand.sort_by_key(|&i| (costs[i].iter().sum::<usize>(), i));
    let mut shares = vec![0; servers.len()];
    let mut left = ceiling;
    for (served, &i) in by_demand.iter().enumerate() {
        let even_split = left / (by_demand.len() - served);
        shares[i] = costs[i].iter().sum::<usize>().min(even_split);
        left -= shares[i];
    }

    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for (((server_id, defs), costs), mut share) in servers.into_iter().zip(costs).zip(shares) {
        let mut dropped_here = Vec::new();
        for (definition, cost) in defs.into_iter().zip(costs) {
            if cost <= share {
                share -= cost;
                kept.push(definition);
            } else {
                dropped_here.push(definition.name);
            }
        }
        if !dropped_here.is_empty() {
            dropped.push(McpDroppedTools {
                server_id,
                tools: dropped_here,
            });
        }
    }
    (kept, dropped)
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

    /// What the model is told after asking for a start. Both outcomes end in
    /// [`CALLABLE_NOW`], because both leave a connected server the rest of
    /// this turn can use.
    pub fn describe(self, id: &str) -> String {
        let state = match self {
            Self::AlreadyRunning { tool_count } => {
                format!("is already running with {tool_count} tools")
            }
            Self::Started { tool_count } => {
                format!("started with {tool_count} tools available")
            }
        };
        format!("MCP server '{id}' {state}, and it is enabled in this thread. {CALLABLE_NOW}")
    }
}

/// An enabled server a turn's setup could not bring up, and why. The turn
/// still runs. The model reads each one as a line of `[MCP SERVERS]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStartFailure {
    pub server_id: String,
    pub error: String,
}

/// What the model is told when it calls a tool of a server its thread has not
/// enabled. Dispatch refuses it, so the array and the gate agree.
fn not_enabled_message(server_id: &str, wire_name: &str) -> String {
    format!(
        "MCP server '{}' is not enabled in this thread, so tool '{}' was NOT run. \
         Call mcp enable with id '{}' first if this task needs it.",
        server_id, wire_name, server_id
    )
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
            tool_surface_generation: Arc::new(AtomicU64::new(0)),
            start_locks: std::sync::Mutex::new(HashMap::new()),
            disabled_tools_writes: Mutex::new(()),
            pool,
            event_bus,
        }
    }

    /// Record that some thread's offered tools may just have changed. Call it
    /// after the change has landed: readers take their stamp before they
    /// read, so a bump after the change can never hide it.
    fn bump_tool_surface(&self) {
        self.tool_surface_generation.fetch_add(1, Ordering::Release);
    }

    /// Take a server out of the registry, recording that the surface moved.
    ///
    /// Stop and Remove differ in what they do with the row, not in what leaves
    /// the registry, so they share this.
    async fn take_running(&self, id: &str) -> Option<RunningServer> {
        let mut running = self.running.write().await;
        let removed = running.remove(id);
        if removed.is_some() {
            self.bump_tool_surface();
        }
        removed
    }

    /// Drop every entry whose process has exited.
    ///
    /// Nothing else notices that an MCP server died. Until this runs the map
    /// still says it is up: its tools ride every request, a start answers
    /// `AlreadyRunning`, and each call fails on a closed pipe.
    ///
    /// The sweep reads under the read guard and takes the write guard only
    /// when it found something, so the usual answer costs no writer. The read
    /// guard is released first, because asking for the write guard while
    /// holding it deadlocks. A restart can land between the two, so every
    /// candidate is probed once more under the write guard, where nothing can
    /// race it.
    async fn evict_exited_servers(&self) {
        let candidates: Vec<String> = {
            let running = self.running.read().await;
            running
                .iter()
                .filter(|(_, entry)| entry.has_exited())
                .map(|(id, _)| id.clone())
                .collect()
        };
        if candidates.is_empty() {
            return;
        }

        let mut running = self.running.write().await;
        let mut evicted = false;
        for id in candidates {
            if running.get(&id).is_some_and(RunningServer::has_exited) {
                running.remove(&id);
                evicted = true;
                log!(
                    "[MCP] Server '{}' exited on its own, dropping it from the registry",
                    id
                );
            }
        }
        if evicted {
            self.bump_tool_surface();
        }
    }

    /// What [`McpManager::tool_surface`] would answer at, without building it.
    ///
    /// The per-round guard in the agentic loop: one atomic load on a turn that
    /// changed nothing, which is nearly every turn.
    pub fn tool_surface_generation(&self) -> u64 {
        self.tool_surface_generation.load(Ordering::Acquire)
    }

    /// Register a new MCP server (saves to DB and connects), and enable it in
    /// `thread_id`, the thread that asked for it.
    pub async fn setup_server(
        &self,
        thread_id: uuid::Uuid,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, crate::core::McpEnvValue>,
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
            Ok(tool_count) => {
                self.emit_enabled(thread_id, id, McpEnablementSource::Agent, EventMeta::NONE)
                    .await?;
                self.bump_tool_surface();
                Ok(format!(
                    "MCP server '{}' is set up, connected with {} tools available, and \
                     enabled in this thread. {}",
                    name, tool_count, CALLABLE_NOW
                ))
            }
            Err(e) => {
                // Server is saved but failed to connect — that's OK, user can start later
                Ok(format!(
                    "MCP server '{}' is registered but failed to connect: {}. \
                     Make sure the server command is correct and any required application (e.g. Blender) is running. \
                     If the command was not found, give an absolute path to it or install its \
                     interpreter (node/npx/uvx/python) in a standard location — a packaged build \
                     searches only a minimal PATH plus the common install dirs. \
                     Call mcp enable to try again.",
                    name, e
                ))
            }
        }
    }

    /// Enable `id` in `thread_id`, starting the server first if it is not
    /// running. A start that fails enables nothing, so the thread never lists
    /// a server it cannot use.
    ///
    /// The generation moves even when the server was already running and
    /// already enabled. Every turn then re-reads its own set, and only the
    /// thread whose slice changed rewrites its array.
    pub async fn enable_for_thread(
        &self,
        thread_id: uuid::Uuid,
        id: &str,
        by: McpEnablementSource,
        meta: EventMeta,
    ) -> Result<McpStartOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let outcome = self.start_server(id).await?;
        self.emit_enabled(thread_id, id, by, meta).await?;
        self.bump_tool_surface();
        Ok(outcome)
    }

    /// Disable `id` in `thread_id` only. The process keeps running for the
    /// threads that still use it. Returns whether the thread had it enabled.
    pub async fn disable_for_thread(
        &self,
        thread_id: uuid::Uuid,
        id: &str,
        by: McpEnablementSource,
        meta: EventMeta,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !self.enabled_servers(thread_id).await.contains(id) {
            return Ok(false);
        }
        self.event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ThreadMcpServerDisabled {
                    server_id: id.to_string(),
                    by,
                },
                meta,
            })
            .await?;
        self.bump_tool_surface();
        Ok(true)
    }

    /// Enable every server a trigger lists, without starting any: the turn's
    /// setup starts them with the rest of the thread's set. Each listed id
    /// no server has comes back as a failure for `[MCP SERVERS]` to report.
    pub async fn enable_listed(
        &self,
        thread_id: uuid::Uuid,
        server_ids: &[String],
    ) -> Vec<McpStartFailure> {
        let mut failures = Vec::new();
        for id in server_ids {
            let error = match McpServerStore::get(&self.pool, id).await {
                Ok(Some(_)) => match self
                    .emit_enabled(thread_id, id, McpEnablementSource::Trigger, EventMeta::NONE)
                    .await
                {
                    Ok(()) => continue,
                    Err(e) => format!("could not be enabled: {e}"),
                },
                Ok(None) => "the trigger lists it, but no MCP server has this id".to_string(),
                Err(e) => format!("could not be read: {e}"),
            };
            failures.push(McpStartFailure {
                server_id: id.clone(),
                error,
            });
        }
        failures
    }

    /// Start every server `thread_id` enabled that is not running, before the
    /// turn's first round. Running does not survive a restart, and enabled
    /// does, so this is where a thread gets its servers back.
    ///
    /// The starts run together, so a server that hangs costs the setup one
    /// connect timeout, not one per server.
    pub async fn start_enabled_servers(&self, thread_id: uuid::Uuid) -> Vec<McpStartFailure> {
        let starts = self
            .enabled_servers(thread_id)
            .await
            .into_iter()
            .map(|id| async move {
                self.start_server(&id).await.err().map(|e| McpStartFailure {
                    server_id: id,
                    error: e.to_string(),
                })
            });
        futures::future::join_all(starts)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Stop a server because someone asked to, which switches it off in every
    /// thread. `None` when no such server is registered.
    ///
    /// The event lands before the process stops. A turn setup after it finds
    /// the server no longer enabled. One already starting it holds the start
    /// lock, which the stop waits for.
    /// Engine shutdown calls [`McpManager::stop_server`] instead, because a
    /// restart must leave every thread's set as it was.
    pub async fn stop_everywhere(
        &self,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<McpStopOutcome>, Box<dyn std::error::Error + Send + Sync>> {
        if McpServerStore::get(&self.pool, id).await?.is_none() {
            return Ok(None);
        }
        self.event_bus
            .emit(BusEvent::System(SystemEvent::McpServerStopped {
                server_id: id.to_string(),
                actor,
            }))
            .await?;
        // A start already past its enabled-set read finishes first, and is
        // then stopped here, rather than landing after the stop.
        let start_lock = self.start_lock(id);
        let _starting = start_lock.lock().await;
        let outcome = self.stop_server(id).await?;
        self.bump_tool_surface();
        Ok(Some(outcome))
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

    /// The lock that orders starts and stops of server `id`.
    fn start_lock(&self, id: &str) -> Arc<Mutex<()>> {
        Arc::clone(
            self.start_locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(id.to_string())
                .or_default(),
        )
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
        let start_lock = self.start_lock(&server.id);
        let _starting = start_lock.lock().await;
        // A server whose process died must not answer `AlreadyRunning`. That
        // answer ends in CALLABLE_NOW, which would be a lie, and a start is
        // the recovery the model is told to reach for.
        self.evict_exited_servers().await;
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
        let env = crate::core::mcp_servers::launch_env(&self.pool, server).await?;
        // MCP servers are invoked as part of LLM tool calls today, so default to Agent.
        // If engine-internal MCP usage is added, that call site should pass ActorMode::Engine.
        let client = McpClient::connect(
            &server.id,
            &server.command,
            &server.args,
            &env,
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

        {
            let mut running = self.running.write().await;
            running.insert(
                server.id.clone(),
                RunningServer {
                    client: Arc::new(Mutex::new(client)),
                    tools,
                    server_config: server.clone(),
                },
            );
            self.bump_tool_surface();
        }

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
        let removed = self.take_running(id).await;
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

    /// Remove a server: delete the row, then stop the process.
    ///
    /// The row goes first, for the reason [`McpManager::stop_everywhere`]
    /// emits first: `McpServerRemoved` clears every thread's enabled set, so
    /// no turn setup can start the server again while it shuts down. The
    /// process is still in the map until then, so nothing is left unnamed.
    ///
    /// Returns whether a row was actually deleted. The caller needs that fact:
    /// reporting "removed" for an id that never existed is a silent success,
    /// and behind a DELETE route it is a 200 that did nothing.
    ///
    /// The process stops even on an error: the row may already be gone, and
    /// a server whose row survived can start again at the next turn setup.
    pub async fn remove_server(
        &self,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let deleted = McpServerStore::unregister(&self.pool, &self.event_bus, id, actor).await;
        if let Some(entry) = self.take_running(id).await {
            entry.client.lock().await.shutdown().await;
        }
        deleted
    }

    /// Switch one of a server's tools off or back on, by wire name.
    /// Returns the stored set, or `None` when no such server exists.
    pub async fn set_tool_disabled(
        &self,
        id: &str,
        wire_name: &str,
        disabled: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
        let _write = self.disabled_tools_writes.lock().await;
        let stored = McpServerStore::set_tool_disabled(
            &self.pool,
            &self.event_bus,
            id,
            wire_name,
            disabled,
            actor,
        )
        .await?;

        // Mirror it onto the running snapshot, which is what the per-request
        // `tool_surface` reads. Without this a tool switched off while the
        // server is up keeps riding every request until the next restart.
        //
        // The generation moves only when the set really changed. Re-submitting
        // the same set offers the same tools, and a turn that rebuilt its
        // array for it would forfeit the prompt cache for identical bytes.
        if let Some(stored) = &stored {
            let mut running = self.running.write().await;
            if let Some(entry) = running.get_mut(id) {
                if entry.server_config.disabled_tools != *stored {
                    entry.server_config.disabled_tools = stored.clone();
                    self.bump_tool_surface();
                }
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
        // `running` and a Live tool list are both read off the map below, so
        // sweep the dead out of it first. Settings showing a server up when
        // its process is gone is the report the user acts on.
        self.evict_exited_servers().await;

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

    /// The refusal for a call from `thread_id` to a server it did not enable,
    /// or `None` when it did. The agentic loop asks this before it shows a
    /// permission card, so nobody is asked to allow a call that cannot run.
    pub async fn not_enabled_refusal(
        &self,
        thread_id: uuid::Uuid,
        server_id: &str,
        wire_name: &str,
    ) -> Option<String> {
        (!self.enabled_servers(thread_id).await.contains(server_id))
            .then(|| not_enabled_message(server_id, wire_name))
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
        thread_id: uuid::Uuid,
        server_id: &str,
        wire_name: &str,
        client: &Arc<Mutex<McpClient>>,
    ) -> Option<String> {
        if let Some(refusal) = self
            .not_enabled_refusal(thread_id, server_id, wire_name)
            .await
        {
            return Some(refusal);
        }
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

    /// Call an MCP tool for `thread_id`. Starts the server on demand if the
    /// thread enabled it and it is not running.
    /// Returns (result_string, server_name, auto_approve).
    pub async fn call_tool(
        &self,
        thread_id: uuid::Uuid,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<(String, String, bool), Box<dyn std::error::Error + Send + Sync>> {
        // Before the on-demand start below, so a thread can never bring up a
        // server it did not enable just by naming one of its tools.
        let wire_name = format!("mcp__{}__{}", server_id, tool_name);
        if let Some(refusal) = self
            .not_enabled_refusal(thread_id, server_id, &wire_name)
            .await
        {
            return Err(refusal.into());
        }

        // A dead process is not a target. Dropping it here is what turns a
        // reuse into the on-demand start below. Reusing it instead fails on a
        // closed pipe, and nothing would ever restart the server.
        self.evict_exited_servers().await;

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
                self.start_loaded(&server).await?;
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
        if let Some(refusal) = self
            .dispatch_refusal(thread_id, server_id, &wire_name, &target.client)
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
                .dispatch_refusal(thread_id, server_id, &wire_name, &target.client)
                .await
            {
                return Err(refusal.into());
            }
            client.call_tool(&tool, arguments).await?
        };

        Ok((result, target.server_name, target.auto_approve))
    }

    /// The tools of every running server `thread_id` enabled, namespaced as
    /// `mcp__{server_id}__{tool_name}` and fitted under `char_ceiling`, and
    /// the generation they were read at.
    ///
    /// Runs on a turn's setup and on any round where the generation moved. It
    /// reads the manifest snapshots, so it never waits on a tool call in flight.
    ///
    /// The stamp is taken FIRST, before the enabled set and the map are read.
    /// Anything that changes after it moves the counter past it, so the turn
    /// looks again next round. A stamp newer than the tools it came with would
    /// skip the very change the turn was watching for.
    pub async fn tool_surface(&self, thread_id: uuid::Uuid, char_ceiling: usize) -> McpToolSurface {
        let generation = self.tool_surface_generation();
        let enabled = self.enabled_servers(thread_id).await;
        let running = self.running.read().await;
        let servers: BTreeSet<String> = running
            .keys()
            .filter(|id| enabled.contains(*id))
            .cloned()
            .collect();
        let (tools, dropped) = fit_to_ceiling(
            offered_by_server(&running, |id| servers.contains(id)),
            char_ceiling,
        );
        drop(running);

        if !dropped.is_empty() {
            log!(
                "[MCP] Tools past the {}-char MCP tool ceiling, not offering them: {}",
                char_ceiling,
                describe_dropped(&dropped)
            );
        }
        McpToolSurface {
            tools,
            generation,
            dropped,
            servers,
        }
    }

    /// What each server would put in the request of a thread that enabled it,
    /// before any ceiling applies: a running server's live tools, a stopped
    /// one's last seen. A server whose id cannot ride the wire is left out,
    /// since nothing can ever enable it.
    async fn offers_if_enabled(
        &self,
    ) -> Result<Vec<(String, Vec<ToolDefinition>)>, Box<dyn std::error::Error + Send + Sync>> {
        self.evict_exited_servers().await;
        let servers = McpServerStore::list(&self.pool).await?;
        let running = self.running.read().await;
        Ok(servers
            .iter()
            .filter(|server| {
                running.contains_key(&server.id)
                    || crate::core::mcp_servers::validate_server_id(&server.id).is_ok()
            })
            .map(|server| {
                let (config, tools) = match running.get(&server.id) {
                    Some(entry) => (&entry.server_config, &*entry.tools),
                    None => (server, server.tools.as_slice()),
                };
                let offered = tool_offers(&server.id, &config.name, tools, &config.disabled_tools)
                    .into_iter()
                    .filter_map(ToolOffer::into_offered)
                    .collect();
                (server.id.clone(), offered)
            })
            .collect())
    }

    /// The worst case against the request budget of a model with
    /// `context_window`: one thread that enabled every server (ADR 0430). The
    /// fit is the one a request applies, so the sent figures are what that
    /// thread's request would carry.
    pub async fn budget(
        &self,
        context_window: usize,
    ) -> Result<McpBudget, Box<dyn std::error::Error + Send + Sync>> {
        let (sent, dropped) = fit_to_ceiling(
            self.offers_if_enabled().await?,
            mcp_tool_char_ceiling(context_window),
        );
        Ok(McpBudget::new(context_window, &sent, dropped))
    }

    /// One `[MCP SERVERS]` line per configured server, stating its status in a
    /// thread that enabled `enabled`. Each `failures` entry no configured
    /// server explains adds a line too, such as a trigger's unknown id.
    ///
    /// A failed read of the registry answers no lines and logs it: the turn
    /// still runs, and the model simply is not told about MCP this turn.
    ///
    /// `trigger_listing` is the firing trigger's `mcp_servers`, or `None` in a
    /// chat thread. A trigger fire may call only what its trigger lists, so a
    /// line inviting it to enable anything else would invite a refusal.
    pub async fn server_lines(
        &self,
        enabled: &BTreeSet<String>,
        failures: &[McpStartFailure],
        trigger_listing: Option<&[String]>,
    ) -> Vec<String> {
        let servers = match self.list_servers().await {
            Ok(servers) => servers,
            Err(e) => {
                log!(
                    "[MCP] Failed to list servers for the turn's server lines: {}",
                    e
                );
                return Vec::new();
            }
        };
        let failure_of = |id: &str| {
            failures
                .iter()
                .find(|f| f.server_id == id)
                .map(|f| f.error.as_str())
        };
        let mut lines: Vec<String> = servers
            .iter()
            .map(|status| {
                let refused_here = trigger_listing
                    .is_some_and(|listed| !status.auto_approve && !listed.contains(&status.id));
                if refused_here {
                    return refused_line(status);
                }
                server_line(status, enabled.contains(&status.id), failure_of(&status.id))
            })
            .collect();
        lines.extend(
            failures
                .iter()
                .filter(|f| !servers.iter().any(|s| s.id == f.server_id))
                .map(|f| format!("- {}: {}.", f.server_id, f.error)),
        );
        lines
    }

    /// The servers `thread_id` enabled for itself, read off the projection.
    ///
    /// A failed read answers the empty set and logs it. An empty set offers
    /// fewer tools, which a turn survives, and dispatch then refuses.
    pub async fn enabled_servers(&self, thread_id: uuid::Uuid) -> BTreeSet<String> {
        match sqlx::query_scalar::<_, Vec<String>>(
            "SELECT enabled_mcp_servers FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(ids) => ids.unwrap_or_default().into_iter().collect(),
            Err(e) => {
                log!(
                    "[MCP] Failed to read the enabled servers of thread {}: {}",
                    thread_id,
                    e
                );
                BTreeSet::new()
            }
        }
    }

    /// Record that `thread_id` enabled `server_id`, unless it already had.
    async fn emit_enabled(
        &self,
        thread_id: uuid::Uuid,
        server_id: &str,
        by: McpEnablementSource,
        meta: EventMeta,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.enabled_servers(thread_id).await.contains(server_id) {
            return Ok(());
        }
        self.event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ThreadMcpServerEnabled {
                    server_id: server_id.to_string(),
                    by,
                },
                meta,
            })
            .await?;
        Ok(())
    }

    /// Give a new child thread its parent's enabled servers. After this the
    /// two sets are independent. Starting them is the child's turn setup.
    pub async fn inherit_enabled(&self, parent: uuid::Uuid, child: uuid::Uuid) {
        for server_id in self.enabled_servers(parent).await {
            if let Err(e) = self
                .emit_enabled(
                    child,
                    &server_id,
                    McpEnablementSource::Parent,
                    EventMeta::NONE,
                )
                .await
            {
                log!(
                    "[MCP] Thread {} did not inherit '{}' from its parent {}: {}",
                    child,
                    server_id,
                    parent,
                    e
                );
            }
        }
    }

    /// Parse a namespaced tool name like "mcp__blender__create_object"
    /// into (server_id, tool_name).
    pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
        let rest = name.strip_prefix("mcp__")?;
        let (server_id, tool_name) = rest.split_once("__")?;
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
    /// caller: `tool_surface` runs over every tool of every server a thread
    /// enabled, and handing back a reference would make it clone each one.
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

/// The definitions each running server `included` keeps would put in a
/// request, before any ceiling applies.
fn offered_by_server(
    running: &HashMap<String, RunningServer>,
    included: impl Fn(&str) -> bool,
) -> Vec<(String, Vec<ToolDefinition>)> {
    running
        .iter()
        .filter(|(server_id, _)| included(server_id))
        .map(|(server_id, entry)| {
            let offered = tool_offers(
                server_id,
                &entry.server_config.name,
                &entry.tools,
                &entry.server_config.disabled_tools,
            )
            .into_iter()
            .filter_map(|offer| {
                if offer.wire_name.is_none() {
                    log!(
                        "[MCP] Tool '{}' on server '{}' has no name that fits the tool-name limit, not offering it",
                        offer.tool.name,
                        server_id
                    );
                }
                offer.into_offered()
            })
            .collect();
            (server_id.clone(), offered)
        })
        .collect()
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

/// How many tools a server offers, or that nobody has seen its list yet.
fn tool_count_phrase(status: &McpServerStatus) -> String {
    if status.tools_source == McpToolsSource::NeverObserved {
        return "tools not yet seen".to_string();
    }
    let offered = status
        .tools
        .iter()
        .filter(|t| !t.disabled && t.wire_name.is_some())
        .count();
    match offered {
        1 => "1 tool".to_string(),
        n => format!("{n} tools"),
    }
}

/// The line for a server a trigger fire may not call: its trigger does not
/// list it, and it has no auto-approve.
fn refused_line(status: &McpServerStatus) -> String {
    format!(
        "- {} ({}): {}. This trigger does not list it, so a call to it is refused in this fire.",
        status.id,
        status.name,
        tool_count_phrase(status)
    )
}

/// One server's line in `[MCP SERVERS]`, as the thread reading it sees it.
/// `failure` is why an enabled server did not start at the turn's setup.
fn server_line(status: &McpServerStatus, enabled: bool, failure: Option<&str>) -> String {
    let label = format!("- {} ({})", status.id, status.name);
    if !status.dispatchable {
        return format!(
            "{label}: its id cannot be used, so it can never run. Tell the user to remove it in Settings."
        );
    }
    if let Some(error) = failure {
        return format!("{label}: enabled in this thread, but it did not start: {error}.");
    }
    let tools = tool_count_phrase(status);
    match (enabled, status.running) {
        (true, true) => format!("{label}: {tools}, enabled in this thread."),
        (true, false) => format!(
            "{label}: {tools}, enabled in this thread but not running. Call mcp enable to start it."
        ),
        (false, true) => {
            format!("{label}: {tools}, running. Call mcp enable to use it here.")
        }
        (false, false) => {
            format!("{label}: {tools}, stopped. Call mcp enable to start it and use it here.")
        }
    }
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
    // The MCP tool ceiling
    // -----------------------------------------------------------------------

    /// `count` tools of about 1.5 KB each, the size a real server ships.
    fn heavy_definitions(server_id: &str, count: usize) -> Vec<ToolDefinition> {
        let names: Vec<String> = (0..count).map(|i| format!("tool_{i:03}")).collect();
        let tools: Vec<McpTool> = names
            .iter()
            .map(|n| McpTool {
                name: n.clone(),
                description: Some(format!("{n}: {}", "Explains itself at length. ".repeat(40))),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "target": { "type": "string", "description": n } },
                })),
            })
            .collect();
        tool_offers(server_id, server_id, &tools, &[])
            .into_iter()
            .filter_map(ToolOffer::into_offered)
            .collect()
    }

    fn names(defs: &[ToolDefinition]) -> Vec<&str> {
        defs.iter().map(|d| d.name.as_str()).collect()
    }

    /// The reported failure: a big `tools/list` took the whole request budget,
    /// so the conversation was trimmed to nothing and then every turn failed.
    #[test]
    fn a_500_tool_manifest_is_bounded_and_leaves_the_conversation_room() {
        let defs = heavy_definitions("github", 500);
        let total_budget = crate::engine::context::agent_context_char_budget(200_000);
        assert!(
            tool_definitions_chars(&defs) > total_budget,
            "the manifest alone must overflow the request, or this proves nothing"
        );

        let ceiling = mcp_tool_char_ceiling(200_000);
        let (kept, dropped) = fit_to_ceiling(vec![("github".to_string(), defs)], ceiling);

        assert!(!kept.is_empty(), "the server keeps what fits");
        assert!(tool_definitions_chars(&kept) <= ceiling);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].server_id, "github");
        assert_eq!(kept.len() + dropped[0].tools.len(), 500);

        let message_budget = total_budget.saturating_sub(tool_definitions_chars(&kept));
        assert!(message_budget > 0);
        assert!(
            message_budget >= total_budget / 4 * 3,
            "the conversation keeps the bulk of the window: {message_budget} of {total_budget}"
        );
    }

    /// A small server is never squeezed to make room for a big one. The big
    /// one keeps its own listed order and loses its tail.
    #[test]
    fn each_server_gets_a_fair_share_and_drops_from_its_own_tail() {
        let small = heavy_definitions("slack", 3);
        let big = heavy_definitions("github", 200);
        let ceiling = tool_definitions_chars(&small) + tool_definitions_chars(&big[..10]);

        let (kept, dropped) = fit_to_ceiling(
            vec![
                ("slack".to_string(), small.clone()),
                ("github".to_string(), big.clone()),
            ],
            ceiling,
        );

        let kept = names(&kept);
        assert_eq!(
            &kept[..10],
            &names(&big[..10])[..],
            "github comes first, by id"
        );
        assert_eq!(&kept[10..], &names(&small)[..], "slack keeps every tool");
        assert_eq!(
            dropped,
            vec![McpDroppedTools {
                server_id: "github".to_string(),
                tools: names(&big[10..]).iter().map(|n| n.to_string()).collect(),
            }]
        );
    }

    /// The array is the first prompt-cache segment, so one map must always
    /// yield one array, whatever order the servers come in.
    #[test]
    fn the_fit_does_not_depend_on_the_order_servers_arrive_in() {
        let a = heavy_definitions("alpha", 40);
        let b = heavy_definitions("beta", 40);
        let ceiling = tool_definitions_chars(&a);

        let forward = fit_to_ceiling(
            vec![
                ("alpha".to_string(), a.clone()),
                ("beta".to_string(), b.clone()),
            ],
            ceiling,
        );
        let backward = fit_to_ceiling(
            vec![("beta".to_string(), b), ("alpha".to_string(), a)],
            ceiling,
        );

        assert_eq!(names(&forward.0), names(&backward.0));
        assert_eq!(forward.1, backward.1);
        assert_eq!(forward.1.len(), 2, "both servers lose their tail");
    }

    #[test]
    fn the_dropped_notice_is_one_line_naming_server_and_tools() {
        let surface = McpToolSurface {
            tools: Vec::new(),
            generation: 0,
            dropped: vec![McpDroppedTools {
                server_id: "github".to_string(),
                tools: (0..25).map(|i| format!("mcp__github__t{i}")).collect(),
            }],
            servers: BTreeSet::from(["github".to_string()]),
        };
        let notice = surface.dropped_notice().expect("tools were dropped");
        assert_eq!(notice.lines().count(), 1, "{notice}");
        assert!(
            notice.contains("server 'github' (25: mcp__github__t0"),
            "{notice}"
        );
        assert!(notice.contains("and 15 more"), "{notice}");
        assert!(!notice.contains("mcp__github__t10"), "{notice}");

        let clean = McpToolSurface {
            dropped: Vec::new(),
            ..surface
        };
        assert_eq!(clean.dropped_notice(), None);
    }

    /// One definition costing exactly `chars`.
    fn definition_of(chars: usize) -> ToolDefinition {
        let with = |description: String| ToolDefinition {
            name: "mcp__srv__t".to_string(),
            description,
            parameters: serde_json::json!({ "type": "object" }),
        };
        let base = tool_definitions_chars(&[with(String::new())]);
        let definition = with("x".repeat(chars - base));
        assert_eq!(
            tool_definitions_chars(std::slice::from_ref(&definition)),
            chars
        );
        definition
    }

    /// The page states the share against the budget a request really has, in
    /// the packer's own unit, and warns before anything is left out.
    #[test]
    fn the_budget_report_agrees_with_the_request_budget_and_warns_near_the_ceiling() {
        let ceiling = mcp_tool_char_ceiling(200_000);
        let quiet = McpBudget::new(200_000, &[definition_of(ceiling / 2)], Vec::new());
        assert_eq!(
            quiet.request_chars,
            crate::engine::context::agent_context_char_budget(200_000)
        );
        assert_eq!(quiet.mcp_ceiling_chars, ceiling);
        assert_eq!((quiet.sent_tools, quiet.sent_chars), (1, ceiling / 2));
        assert_eq!(quiet.sent_tokens, estimate_tokens_from_chars(ceiling / 2));
        assert_eq!(quiet.warning, None);

        let near = McpBudget::new(200_000, &[definition_of(ceiling / 10 * 9)], Vec::new());
        let warning = near.warning.expect("90% of the ceiling warns");
        assert!(warning.contains("90%"), "{warning}");

        let past = McpBudget::new(
            200_000,
            &[definition_of(ceiling)],
            vec![McpDroppedTools {
                server_id: "github".to_string(),
                tools: vec!["mcp__github__a".to_string(), "mcp__github__b".to_string()],
            }],
        );
        let warning = past.warning.expect("dropped tools warn");
        assert!(
            warning.starts_with("A thread that enables every server leaves 2 tools out"),
            "{warning}"
        );
        assert!(warning.contains("server 'github'"), "{warning}");

        assert_eq!(McpBudget::new(200_000, &[], Vec::new()).warning, None);
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

    /// The invariant "no em dash": every `[MCP SERVERS]` line states the
    /// reading thread's own status, and none carries U+2014 (the old stopped
    /// line did).
    #[test]
    fn each_server_line_states_the_threads_own_status_without_an_em_dash() {
        let tools = priced_tools(&["alpha", "beta"]);
        let status = |running: bool, disabled: &[&str]| {
            server_status(
                &server_row("slack", disabled),
                &tools,
                if running {
                    McpToolsSource::Live
                } else {
                    McpToolsSource::Cache
                },
                running,
            )
        };
        let lines = [
            server_line(&status(true, &[]), true, None),
            server_line(&status(true, &["mcp__slack__beta"]), false, None),
            server_line(&status(false, &[]), false, None),
            server_line(&status(false, &[]), true, Some("spawn failed")),
            server_line(&status(false, &[]), true, None),
        ];
        assert_eq!(
            lines[0],
            "- slack (slack server): 2 tools, enabled in this thread."
        );
        assert_eq!(
            lines[1],
            "- slack (slack server): 1 tool, running. Call mcp enable to use it here."
        );
        assert_eq!(
            lines[2],
            "- slack (slack server): 2 tools, stopped. Call mcp enable to start it and use it here."
        );
        assert_eq!(
            lines[3],
            "- slack (slack server): enabled in this thread, but it did not start: spawn failed."
        );
        assert!(lines[4].contains("enabled in this thread but not running"));

        let mut unseen = server_row("github", &[]);
        unseen.tools_observed_at = None;
        let unseen = server_status(&unseen, &[], McpToolsSource::NeverObserved, false);
        assert!(server_line(&unseen, false, None).contains("tools not yet seen, stopped"));

        let unusable = server_status(
            &server_row("back.stage", &[]),
            &[],
            McpToolsSource::Cache,
            false,
        );
        assert!(server_line(&unusable, true, None).contains("can never run"));

        for line in lines.iter() {
            assert!(!line.contains('\u{2014}'), "{line}");
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
if [ -n "$STUB_GRANDCHILD_PID_FILE" ]; then
  sleep 300 </dev/null >/dev/null 2>&1 &
  echo $! > "$STUB_GRANDCHILD_PID_FILE"
fi
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
        grandchild_pid_file: std::path::PathBuf,
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
            let grandchild_pid_file = dir.path().join("grandchild.pid");
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
                grandchild_pid_file,
            }
        }

        /// The env as a registration stores it: every entry a plain value.
        fn registered_env(&self) -> HashMap<String, crate::core::McpEnvValue> {
            self.env
                .iter()
                .map(|(var, value)| (var.clone(), crate::core::McpEnvValue::Plain(value.clone())))
                .collect()
        }

        /// Start a long `sleep` beside the server, the shape of an `npx` or
        /// `uvx` launcher whose interpreter outlives a kill aimed at it alone.
        fn spawns_a_grandchild(mut self) -> Self {
            self.env.insert(
                "STUB_GRANDCHILD_PID_FILE".to_string(),
                self.grandchild_pid_file.display().to_string(),
            );
            self
        }

        /// Block until the grandchild is gone. On timeout it is killed before
        /// the panic, so a failing run leaves no `sleep` behind. Only ever the
        /// pid this stub itself wrote, and only while it is still running.
        async fn await_grandchild_stop(&self) {
            let pid = std::fs::read_to_string(&self.grandchild_pid_file)
                .expect("the stub wrote its grandchild's pid");
            let pid = pid.trim().to_string();
            for _ in 0..250 {
                if pid_has_stopped(&pid) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            let _ = std::process::Command::new("kill")
                .args(["-KILL", pid.as_str()])
                .status();
            panic!("the server's grandchild {pid} outlived the stop");
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
        /// with a group kill on every path that abandons a client.
        async fn await_exit(&self) {
            for _ in 0..150 {
                if !self.is_alive() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the stub outlived the client that spawned it");
        }

        /// Kill this stub's process, standing in for a server that dies on its
        /// own. Only ever the pid this stub itself wrote.
        ///
        /// Waits for the process to stop rather than for `kill` to return:
        /// SIGKILL marks the target, and the exit follows. A stopped child is
        /// a zombie until its parent reaps it, so `Z` counts as stopped and
        /// `kill -0` cannot be the signal here.
        async fn kill_and_await_stop(&self) {
            let pid = std::fs::read_to_string(&self.pid_file).expect("the stub wrote its pid");
            let pid = pid.trim().to_string();
            let signalled = std::process::Command::new("kill")
                .args(["-KILL", pid.as_str()])
                .status()
                .expect("run kill");
            assert!(signalled.success(), "the stub pid {pid} was not signalled");

            for _ in 0..250 {
                if pid_has_stopped(&pid) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the stub kept running after SIGKILL");
        }
    }

    /// Whether `pid` has stopped running, reading the process state directly.
    /// Empty output is a pid that is gone, and `Z` is one waiting to be reaped.
    fn pid_has_stopped(pid: &str) -> bool {
        let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
        else {
            return false;
        };
        let state = String::from_utf8_lossy(&output.stdout);
        let state = state.trim();
        state.is_empty() || state.starts_with('Z')
    }

    async fn manager_with(
        pool: &sqlx::PgPool,
        id: &str,
        stub: &StubServer,
    ) -> (McpManager, crate::engine::event_bus::EventBus) {
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        McpServerStore::register(
            pool,
            &bus,
            id,
            "Stub",
            &stub.command,
            &[],
            &stub.registered_env(),
            None,
        )
        .await
        .unwrap();
        (McpManager::new(pool.clone(), bus.clone()), bus)
    }

    /// One manager over several stub servers, for the tests that need two
    /// processes at once.
    /// A chat thread that enabled `server_ids`, recorded as events alone: no
    /// server starts, and the generation does not move.
    async fn thread_enabling(manager: &McpManager, server_ids: &[&str]) -> uuid::Uuid {
        let thread_id =
            crate::test_support::seed_chat_thread(&manager.event_bus, "use the stub").await;
        for id in server_ids {
            manager
                .emit_enabled(thread_id, id, McpEnablementSource::User, EventMeta::NONE)
                .await
                .unwrap();
        }
        thread_id
    }

    async fn manager_with_all(pool: &sqlx::PgPool, stubs: &[(&str, &StubServer)]) -> McpManager {
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        for (id, stub) in stubs {
            McpServerStore::register(
                pool,
                &bus,
                id,
                id,
                &stub.command,
                &[],
                &stub.registered_env(),
                None,
            )
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

    /// The ceiling applies to what a running server really offers, and the
    /// cost report sees the same cut the request does.
    #[tokio::test]
    async fn a_running_server_past_the_ceiling_is_cut_and_the_report_says_so() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta", "gamma"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        let all = manager.tool_surface(thread, usize::MAX).await;
        assert!(all.dropped.is_empty());
        let one_tool = tool_definitions_chars(&all.tools[..1]);

        let cut = manager.tool_surface(thread, one_tool).await;
        assert_eq!(cut.tools.len(), 1);
        assert_eq!(cut.dropped.len(), 1);
        assert_eq!(cut.dropped[0].tools.len(), 2);
        assert!(cut.dropped_notice().is_some());

        // A window small enough that its ceiling fits no tool at all.
        let budget = manager.budget(0).await.unwrap();
        assert_eq!(budget.mcp_ceiling_chars, 0);
        assert_eq!((budget.sent_tools, budget.sent_chars), (0, 0));
        assert_eq!(budget.dropped[0].tools.len(), 3);
        assert!(budget.warning.is_some());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The cost report's budget is the worst case, one thread that enabled
    /// every server (ADR 0430): a stopped server counts with the tools it last
    /// offered, and one nobody has seen counts nothing.
    #[tokio::test]
    async fn the_budget_counts_every_server_as_one_thread_enabling_them_all() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let up = StubServer::new(&priced_tools(&["alpha"]));
        let down = StubServer::new(&priced_tools(&["beta", "gamma"]));
        let unseen = StubServer::new(&priced_tools(&["delta"]));
        let manager =
            manager_with_all(&pool, &[("up", &up), ("down", &down), ("unseen", &unseen)]).await;
        manager.start_server("down").await.unwrap();
        manager.stop_server("down").await.unwrap();
        manager.start_server("up").await.unwrap();

        let budget = manager.budget(200_000).await.unwrap();
        assert_eq!(budget.sent_tools, 3, "up's one tool and down's two");
        assert!(budget.dropped.is_empty());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Switching a tool off has to reach the definitions the NEXT request
    /// carries, not just the DB. The running snapshot is what that path reads.
    #[tokio::test]
    async fn disabling_a_tool_removes_it_from_a_running_server_s_definitions() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        let offered = |surface: McpToolSurface| -> Vec<String> {
            let mut names: Vec<String> = surface.tools.into_iter().map(|d| d.name).collect();
            names.sort();
            names
        };
        assert_eq!(
            offered(manager.tool_surface(thread, usize::MAX).await),
            vec![
                "mcp__stub__alpha".to_string(),
                "mcp__stub__beta".to_string()
            ]
        );

        manager
            .set_tool_disabled("stub", "mcp__stub__beta", true, None)
            .await
            .unwrap()
            .expect("the server exists");

        assert_eq!(
            offered(manager.tool_surface(thread, usize::MAX).await),
            vec!["mcp__stub__alpha".to_string()],
            "a disabled tool must leave the request without a restart"
        );

        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(status.tools.iter().any(|t| t.disabled));
        assert!(status.disabled_chars > 0);

        // Switching it back on brings it straight back.
        manager
            .set_tool_disabled("stub", "mcp__stub__beta", false, None)
            .await
            .unwrap()
            .expect("the server exists");
        assert_eq!(
            manager.tool_surface(thread, usize::MAX).await.tools.len(),
            2
        );

        assert!(manager
            .set_tool_disabled("missing", "mcp__missing__x", false, None)
            .await
            .unwrap()
            .is_none());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Two switches on one running server, sent at once, both reach the stored
    /// set and the running snapshot. The write lock keeps the snapshot copy in
    /// step with the database, so dispatch and the listing agree (ADR 0437).
    #[tokio::test]
    async fn two_switches_at_once_both_reach_the_running_snapshot() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        let (alpha, beta) = tokio::join!(
            manager.set_tool_disabled("stub", "mcp__stub__alpha", true, None),
            manager.set_tool_disabled("stub", "mcp__stub__beta", true, None),
        );
        alpha.unwrap().expect("the server exists");
        beta.unwrap().expect("the server exists");

        assert!(
            manager
                .tool_surface(thread, usize::MAX)
                .await
                .tools
                .is_empty(),
            "both tools leave the request"
        );
        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(
            status.tools.iter().all(|t| t.disabled),
            "both read as off in the listing"
        );

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
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        manager
            .set_tool_disabled("stub", "mcp__stub__beta", true, None)
            .await
            .unwrap()
            .expect("the server exists");

        // Calling it the way an in-flight tool call would, by the wire name the
        // model was shown before the switch.
        let refused = manager
            .call_tool(thread, "stub", "beta", serde_json::json!({}))
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
            .call_tool(thread, "stub", "alpha", serde_json::json!({}))
            .await
            .expect("an enabled tool still dispatches");
        assert_eq!(result, "stub ran");
        assert_eq!(server_name, "Stub");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Remove leaves no process behind, and no row.
    #[tokio::test]
    async fn removing_a_running_server_stops_its_process() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();
        assert!(stub.is_alive(), "the stub should be up before removal");

        assert!(manager.remove_server("stub", None).await.unwrap());

        assert!(!stub.is_alive(), "the process must not outlive its row");
        assert!(McpServerStore::get(&pool, "stub").await.unwrap().is_none());
        assert!(
            manager
                .tool_surface(thread, usize::MAX)
                .await
                .tools
                .is_empty(),
            "a removed server must not keep offering tools"
        );
        assert!(manager.list_servers().await.unwrap().is_empty());

        // Removing again removes nothing, which is what the route turns into a
        // 404 rather than a silent success.
        assert!(!manager.remove_server("stub", None).await.unwrap());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Three routes bring a server up, and all three end a turn the model can
    /// still act in. Leaving one silent is what produced the reported failure:
    /// the agent read "started" as a fact about the next turn and deferred the
    /// work it had just enabled.
    #[tokio::test]
    async fn every_way_a_server_comes_up_tells_the_model_its_tools_are_live_now() {
        assert!(McpStartOutcome::Started { tool_count: 15 }
            .describe("slack")
            .contains(CALLABLE_NOW));
        assert!(McpStartOutcome::AlreadyRunning { tool_count: 15 }
            .describe("slack")
            .contains(CALLABLE_NOW));

        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let registered = manager
            .setup_server(
                crate::test_support::seed_chat_thread(&manager.event_bus, "set it up").await,
                "fresh",
                "Fresh",
                &stub.command,
                &[],
                &stub.registered_env(),
            )
            .await
            .unwrap();
        assert!(
            registered.contains(CALLABLE_NOW),
            "setup connects too, so it says the same: {registered}"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// An MCP server can die with nobody watching, and its entry used to
    /// outlive it. Every one of the three readers below then answered off a
    /// process that was gone.
    ///
    /// Each step kills the stub and immediately exercises one reader, so no
    /// earlier step can be what swept the entry out.
    #[tokio::test]
    async fn a_server_whose_process_died_is_evicted_before_anything_trusts_it() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();
        let live = manager.tool_surface(thread, usize::MAX).await;
        assert_eq!(live.tools.len(), 2);

        // A start is the recovery the model is told to reach for.
        // `AlreadyRunning` ends in CALLABLE_NOW, which a dead server makes a
        // lie.
        stub.kill_and_await_stop().await;
        assert_eq!(
            manager.start_server("stub").await.unwrap(),
            McpStartOutcome::Started { tool_count: 2 },
            "bringing a dead server back is a start, never AlreadyRunning"
        );

        // A call must restart it rather than dispatch into a closed pipe.
        stub.kill_and_await_stop().await;
        let (result, _name, _auto_approve) = manager
            .call_tool(thread, "stub", "alpha", serde_json::json!({}))
            .await
            .expect("a call on a dead server restarts it");
        assert_eq!(result, "stub ran");

        // Settings reads the process, not the last thing the map was told.
        stub.kill_and_await_stop().await;
        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(!status.running, "a dead process is not a running server");
        assert_eq!(
            status.tools_source,
            McpToolsSource::Cache,
            "its manifest is remembered now, not read off the process"
        );

        let dropped = manager.tool_surface(thread, usize::MAX).await;
        assert!(
            dropped.tools.is_empty(),
            "a dead server must stop costing tokens on every request"
        );
        assert_ne!(
            dropped.generation, live.generation,
            "a turn in flight has to see that the surface moved"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The generation is what a running turn watches, so it has to move on
    /// exactly the changes the model would notice and on nothing else. A miss
    /// leaves a started server uncallable for the rest of the turn. A false
    /// move rebuilds the array and forfeits the prompt cache for nothing.
    #[tokio::test]
    async fn the_tool_surface_generation_moves_on_every_change_and_on_no_no_op() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;

        let before_start = manager.tool_surface_generation();
        manager.start_server("stub").await.unwrap();
        let started = manager.tool_surface(thread, usize::MAX).await;
        assert_eq!(started.tools.len(), 2);
        assert_ne!(
            started.generation, before_start,
            "a started server is a new tool surface"
        );
        assert_eq!(
            started.generation,
            manager.tool_surface_generation(),
            "the cheap read and the built surface must agree"
        );

        // Starting what is already running changes nothing, so a turn that
        // tried it must not pay a rebuild.
        assert_eq!(
            manager.start_server("stub").await.unwrap(),
            McpStartOutcome::AlreadyRunning { tool_count: 2 }
        );
        assert_eq!(manager.tool_surface_generation(), started.generation);

        manager
            .set_tool_disabled("stub", "mcp__stub__beta", true, None)
            .await
            .unwrap()
            .expect("the server exists");
        let disabled = manager.tool_surface(thread, usize::MAX).await;
        assert_eq!(disabled.tools.len(), 1);
        assert_ne!(disabled.generation, started.generation);

        // Switching it off again, and a write for a server nobody started,
        // both leave the offered tools exactly as they were.
        manager
            .set_tool_disabled("stub", "mcp__stub__beta", true, None)
            .await
            .unwrap()
            .expect("the server exists");
        manager
            .set_tool_disabled("missing", "mcp__missing__x", false, None)
            .await
            .unwrap();
        assert_eq!(manager.tool_surface_generation(), disabled.generation);

        manager.stop_server("stub").await.unwrap();
        let stopped = manager.tool_surface(thread, usize::MAX).await;
        assert!(
            stopped.tools.is_empty(),
            "a stopped server stops being callable"
        );
        assert_ne!(stopped.generation, disabled.generation);

        // Stopping it again, and then removing a server that was already
        // stopped, take nothing out of the array.
        manager.stop_server("stub").await.unwrap();
        assert!(manager.remove_server("stub", None).await.unwrap());
        assert_eq!(manager.tool_surface_generation(), stopped.generation);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    // -----------------------------------------------------------------------
    // Enabled per thread (ADR 0430)
    // -----------------------------------------------------------------------

    fn offered_names(surface: &McpToolSurface) -> Vec<String> {
        let mut names: Vec<String> = surface.tools.iter().map(|d| d.name.clone()).collect();
        names.sort();
        names
    }

    async fn events_named(pool: &sqlx::PgPool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The invariant "dispatch agrees": a thread that did not enable a running
    /// server cannot call its tools. Naming a tool does not start the server
    /// on demand either.
    #[tokio::test]
    async fn a_thread_that_did_not_enable_a_server_cannot_call_its_tools() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();
        let bystander = crate::test_support::seed_chat_thread(&bus, "unrelated").await;

        assert!(manager
            .tool_surface(bystander, usize::MAX)
            .await
            .tools
            .is_empty());
        let refused = manager
            .call_tool(bystander, "stub", "alpha", serde_json::json!({}))
            .await
            .expect_err("a server the thread did not enable must not dispatch")
            .to_string();
        assert!(refused.contains("mcp enable"), "{refused}");
        assert!(refused.contains("NOT run"), "{refused}");
        assert!(!stub.call_started(), "the call must not reach the process");

        manager.stop_server("stub").await.unwrap();
        assert!(manager
            .call_tool(bystander, "stub", "alpha", serde_json::json!({}))
            .await
            .is_err());
        assert!(
            !stub.is_alive(),
            "naming a tool must not start a server the thread never enabled"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The invariant "no cross-thread change": thread A enables a server in
    /// the middle of a turn. The generation moves for every running turn, and
    /// thread B recomputes the very slice it held, byte for byte. `TurnTools`
    /// then rewrites nothing (`a_moved_generation_with_the_same_slice_rewrites_nothing`).
    #[tokio::test]
    async fn one_thread_enabling_a_server_changes_no_other_threads_slice() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;
        let a = crate::test_support::seed_chat_thread(&bus, "check the stub").await;
        let b = crate::test_support::seed_chat_thread(&bus, "something else").await;
        let b_before = manager.tool_surface(b, usize::MAX).await;

        manager
            .enable_for_thread(a, "stub", McpEnablementSource::Agent, EventMeta::NONE)
            .await
            .unwrap();
        assert_ne!(
            manager.tool_surface_generation(),
            b_before.generation,
            "every running turn has to look again"
        );

        assert_eq!(
            offered_names(&manager.tool_surface(a, usize::MAX).await),
            ["mcp__stub__alpha", "mcp__stub__beta"]
        );
        let b_after = manager.tool_surface(b, usize::MAX).await;
        assert_eq!(
            serde_json::to_string(&b_after.tools).unwrap(),
            serde_json::to_string(&b_before.tools).unwrap()
        );
        assert_eq!(b_after.servers, b_before.servers);
        assert_eq!(b_after.dropped, b_before.dropped);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The invariant "enabled survives a restart, and running does not". A
    /// fresh manager stands in for the restarted engine: nothing runs at boot,
    /// and the turn's setup brings the thread's servers back.
    #[tokio::test]
    async fn a_threads_enabled_servers_come_back_at_its_next_turn_after_a_restart() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (before_restart, bus) = manager_with(&pool, "stub", &stub).await;
        let thread_id = crate::test_support::seed_chat_thread(&bus, "use the stub").await;
        before_restart
            .enable_for_thread(
                thread_id,
                "stub",
                McpEnablementSource::Agent,
                EventMeta::NONE,
            )
            .await
            .unwrap();
        // What shutdown does: every process ends, and nothing is recorded.
        before_restart.stop_server("stub").await.unwrap();
        drop(before_restart);
        assert_eq!(events_named(&pool, "McpServerStopped").await, 0);

        let after_restart = McpManager::new(pool.clone(), bus.clone());
        assert!(
            !after_restart.list_servers().await.unwrap()[0].running,
            "nothing starts a server at boot"
        );
        assert!(after_restart
            .start_enabled_servers(thread_id)
            .await
            .is_empty());
        assert!(stub.is_alive());
        assert_eq!(
            offered_names(&after_restart.tool_surface(thread_id, usize::MAX).await),
            ["mcp__stub__alpha"]
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The invariant "stop reaches every thread": two threads enabled a
    /// server, and one stop leaves neither offering it, in the projection or
    /// in the slice. Disabling, by contrast, touches one thread.
    #[tokio::test]
    async fn a_stop_switches_a_server_off_in_every_thread_and_a_disable_in_one() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;
        let first = crate::test_support::seed_chat_thread(&bus, "one").await;
        let second = crate::test_support::seed_chat_thread(&bus, "two").await;
        for thread_id in [first, second] {
            manager
                .enable_for_thread(
                    thread_id,
                    "stub",
                    McpEnablementSource::Agent,
                    EventMeta::NONE,
                )
                .await
                .unwrap();
        }

        assert!(manager
            .disable_for_thread(first, "stub", McpEnablementSource::User, EventMeta::NONE)
            .await
            .unwrap());
        assert!(manager
            .tool_surface(first, usize::MAX)
            .await
            .tools
            .is_empty());
        assert_eq!(
            manager.tool_surface(second, usize::MAX).await.tools.len(),
            1
        );
        assert!(
            stub.is_alive(),
            "a disable leaves the process to the others"
        );
        manager
            .enable_for_thread(first, "stub", McpEnablementSource::Agent, EventMeta::NONE)
            .await
            .unwrap();

        let before = manager.tool_surface_generation();
        assert!(matches!(
            manager.stop_everywhere("stub", None).await.unwrap(),
            Some(McpStopOutcome::Stopped { .. })
        ));
        assert_ne!(manager.tool_surface_generation(), before);
        for thread_id in [first, second] {
            assert!(manager.enabled_servers(thread_id).await.is_empty());
            assert!(manager
                .tool_surface(thread_id, usize::MAX)
                .await
                .tools
                .is_empty());
        }
        assert_eq!(events_named(&pool, "McpServerStopped").await, 1);
        assert!(manager.start_enabled_servers(first).await.is_empty());
        assert!(!stub.is_alive(), "the next turn must not start it again");
        assert_eq!(
            manager.stop_everywhere("missing", None).await.unwrap(),
            None
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The invariant "a trigger that lists a server has its schemas from
    /// round 1", at the setup half: the fire enables what the trigger lists,
    /// the setup starts it, and the first surface already carries its tools.
    /// An id no server has becomes a line of `[MCP SERVERS]`.
    #[tokio::test]
    async fn a_trigger_listed_server_is_in_the_turns_first_surface() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;
        let thread_id = crate::test_support::seed_chat_thread(&bus, "hourly watch").await;

        let mut failures = manager
            .enable_listed(thread_id, &["stub".to_string(), "ghost".to_string()])
            .await;
        failures.extend(manager.start_enabled_servers(thread_id).await);

        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(failures[0].server_id, "ghost");
        assert_eq!(
            offered_names(&manager.tool_surface(thread_id, usize::MAX).await),
            ["mcp__stub__alpha"]
        );
        let lines = manager
            .server_lines(
                &manager.enabled_servers(thread_id).await,
                &failures,
                Some(&["stub".to_string(), "ghost".to_string()]),
            )
            .await;
        assert_eq!(
            lines,
            [
                "- stub (Stub): 1 tool, enabled in this thread.",
                "- ghost: the trigger lists it, but no MCP server has this id.",
            ]
        );

        // A fire whose trigger does not list the server is told its calls are
        // refused, rather than invited to enable it.
        let unlisted = manager
            .server_lines(&BTreeSet::new(), &[], Some(&["other".to_string()]))
            .await;
        assert_eq!(
            unlisted,
            ["- stub (Stub): 1 tool. This trigger does not list it, so a call to it is refused in this fire."]
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Two threads waking together both start their shared server. One start
    /// spawns it, and the other finds it running, so one process serves both.
    #[tokio::test]
    async fn concurrent_starts_of_one_server_spawn_one_process() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;

        let (first, second) =
            tokio::join!(manager.start_server("stub"), manager.start_server("stub"));
        let mut outcomes = [first.unwrap(), second.unwrap()];
        outcomes.sort_by_key(|o| matches!(o, McpStartOutcome::AlreadyRunning { .. }));
        assert_eq!(
            outcomes,
            [
                McpStartOutcome::Started { tool_count: 1 },
                McpStartOutcome::AlreadyRunning { tool_count: 1 }
            ]
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Enabling twice records one event. A start that fails enables nothing,
    /// so a thread never lists a server it cannot use.
    #[tokio::test]
    async fn enabling_is_idempotent_and_a_failed_start_enables_nothing() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;
        McpServerStore::register(
            &pool,
            &bus,
            "broken",
            "Broken",
            "/nonexistent/mcp-server",
            &[],
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();
        let thread_id = crate::test_support::seed_chat_thread(&bus, "go").await;

        for _ in 0..2 {
            let outcome = manager
                .enable_for_thread(
                    thread_id,
                    "stub",
                    McpEnablementSource::Agent,
                    EventMeta::NONE,
                )
                .await
                .unwrap();
            assert!(outcome.describe("stub").contains("enabled in this thread"));
        }
        assert_eq!(events_named(&pool, "ThreadMcpServerEnabled").await, 1);

        assert!(manager
            .enable_for_thread(
                thread_id,
                "broken",
                McpEnablementSource::Agent,
                EventMeta::NONE
            )
            .await
            .is_err());
        assert!(manager
            .enable_for_thread(
                thread_id,
                "missing",
                McpEnablementSource::Agent,
                EventMeta::NONE
            )
            .await
            .is_err());
        assert_eq!(
            manager.enabled_servers(thread_id).await,
            BTreeSet::from(["stub".to_string()])
        );

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
        let thread = thread_enabling(&manager, &["slow", "quick"]).await;
        manager.start_server("slow").await.unwrap();
        manager.start_server("quick").await.unwrap();

        let budget = std::time::Duration::from_millis(1500);
        let busy = manager.call_tool(thread, "slow", "alpha", serde_json::json!({}));
        let meanwhile = async {
            slow.await_call_started().await;
            let answered = tokio::time::timeout(
                budget,
                manager.call_tool(thread, "quick", "beta", serde_json::json!({})),
            )
            .await
            .expect("a call to another server must not wait for the slow one")
            .expect("the quick server answers");
            let surface = tokio::time::timeout(budget, manager.tool_surface(thread, usize::MAX))
                .await
                .expect("tool assembly must not wait for a call in flight");
            (answered, surface)
        };
        let (slow_result, (answered, surface)) = tokio::join!(busy, meanwhile);

        assert_eq!(answered.0, "stub ran");
        assert_eq!(surface.tools.len(), 2, "both servers are still offered");
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
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        let started = std::time::Instant::now();
        let (first, second) = tokio::join!(
            manager.call_tool(thread, "stub", "alpha", serde_json::json!({})),
            manager.call_tool(thread, "stub", "beta", serde_json::json!({})),
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
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        let busy = manager.call_tool(thread, "stub", "alpha", serde_json::json!({}));
        let meanwhile = async {
            stub.await_call_started().await;
            // Queued behind the call in flight, and switched off while it
            // waits. It passes the arrival check and must fail the dispatch
            // one.
            let queued = manager.call_tool(thread, "stub", "beta", serde_json::json!({}));
            let switch_off = async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                manager
                    .set_tool_disabled("stub", "mcp__stub__beta", true, None)
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
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();

        let busy = manager.call_tool(thread, "stub", "alpha", serde_json::json!({}));
        let meanwhile = async {
            stub.await_call_started().await;
            let queued = manager.call_tool(thread, "stub", "beta", serde_json::json!({}));
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
        let thread = thread_enabling(&manager, &["flood"]).await;
        manager.start_server("flood").await.unwrap();

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            manager.call_tool(thread, "flood", "alpha", serde_json::json!({})),
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
        let thread = thread_enabling(&manager, &["broken"]).await;

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
        assert!(manager
            .tool_surface(thread, usize::MAX)
            .await
            .tools
            .is_empty());

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
            &empty.registered_env(),
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

    /// Stop reaches everything the server started, not only the server.
    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_a_server_stops_the_processes_it_started() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&[]).spawns_a_grandchild();
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();

        manager.stop_server("stub").await.unwrap();

        stub.await_exit().await;
        stub.await_grandchild_stop().await;

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// An abandoned client takes the whole process group with it too, since
    /// `Drop` is the only teardown on a failed start.
    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_a_client_stops_the_processes_its_server_started() {
        let stub = StubServer::new(&[]).spawns_a_grandchild();
        let client = McpClient::connect("stub", &stub.command, &[], &stub.env, ActorMode::Agent)
            .await
            .unwrap();

        drop(client);

        stub.await_exit().await;
        stub.await_grandchild_stop().await;
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
