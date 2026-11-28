pub mod client;
pub mod types;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::core::{LaunchLineMode, McpServer, McpServerStore};
use crate::engine::context::{estimate_tokens_from_chars, tool_definitions_chars};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::{
    ActorMode, EventMeta, McpEnablementSource, MessageOrigin, ThreadEvent,
};
use crate::llm::provider::ToolDefinition;
use client::McpClient;
use types::McpTool;

struct RunningServer {
    client: Arc<Mutex<McpClient>>,
    tools: Arc<[McpTool]>,
    /// INVARIANT: The registry row as it stood when the process started, kept in step for
    /// the fields the request path reads: `auto_approve` and `disabled_tools`.
    /// `tools` on it is NOT maintained, because a running server's tools are
    /// the snapshot above and a stopped one's come off a fresh DB row.
    server_config: McpServer,
}

/// INVARIANT: What one dispatch takes off the registry, so the guard is released before
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
    /// INVARIANT: Whether this server's process is known to be gone.
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

pub const CALLABLE_NOW: &str = "Its tools are callable right now, in this same turn: \
     use them instead of asking the user to send another message.";

fn disabled_tool_refusal(server_id: &str, wire_name: &str) -> String {
    format!(
        "MCP tool '{}' is switched off for server '{}' and was NOT run. \
         Do not retry it. Tell the user it is disabled, and let them \
         re-enable it in Settings if they want it back.",
        wire_name, server_id
    )
}

pub struct McpManager {
    /// INVARIANT: Currently running servers keyed by server id.
    ///
    /// The guard covers map work only, never an await into a server process.
    /// It used to be held across the tool call itself. One slow server then
    /// stalled tool assembly for every thread in the workspace, and two
    /// servers could never work at once.
    running: Arc<RwLock<HashMap<String, RunningServer>>>,
    /// INVARIANT: Moves whenever [`McpManager::tool_surface`] would answer differently.
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
    /// INVARIANT: One lock per server id, held from the "is it running" check to the
    /// insert. Every turn setup starts its thread's servers, so without it two
    /// threads waking together spawn two processes for one server.
    start_locks: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// INVARIANT: Held from a disabled-tools write to its copy onto the running snapshot.
    /// Without it two writes could commit in one order and copy in the other,
    /// and dispatch would then disagree with the stored set.
    disabled_tools_writes: Mutex<()>,
    pool: sqlx::PgPool,
    event_bus: crate::engine::event_bus::EventBus,
    workspace_root: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpToolsSource {
    Live,
    Cache,
    NeverObserved,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct McpToolStatus {
    pub name: String,
    pub wire_name: Option<String>,
    pub description: Option<String>,
    pub disabled: bool,
    pub chars: usize,
    pub tokens: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub auto_approve: bool,
    pub dispatchable: bool,
    pub launch_line: Option<String>,
    pub launch_approved: bool,
    pub tools_source: McpToolsSource,
    pub tools_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub tools: Vec<McpToolStatus>,
    pub chars: usize,
    pub tokens: usize,
    pub disabled_chars: usize,
    pub disabled_tokens: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct McpCostTotals {
    pub servers: usize,
    pub running_servers: usize,
    pub tools: usize,
    pub chars: usize,
    pub tokens: usize,
    pub stopped_tools: usize,
    pub stopped_chars: usize,
    pub stopped_tokens: usize,
    pub disabled_tools: usize,
    pub disabled_chars: usize,
    pub disabled_tokens: usize,
}

impl McpCostTotals {
    pub fn of(servers: &[McpServerStatus]) -> Self {
        let mut totals = Self {
            servers: servers.len(),
            ..Self::default()
        };
        for server in servers {
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

pub struct McpToolSurface {
    pub tools: Vec<ToolDefinition>,
    pub generation: u64,
    pub dropped: Vec<McpDroppedTools>,
    pub servers: BTreeSet<String>,
}

impl McpToolSurface {
    pub fn dropped_notice(&self) -> Option<String> {
        dropped_notice(&self.dropped)
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct McpDroppedTools {
    pub server_id: String,
    pub tools: Vec<String>,
}

const DROPPED_NAMES_SHOWN: usize = 10;

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

const MCP_BUDGET_DIVISOR: usize = 4;

pub fn mcp_tool_char_ceiling(context_window: usize) -> usize {
    crate::engine::context::agent_context_char_budget(context_window) / MCP_BUDGET_DIVISOR
}

pub fn mcp_tool_char_ceiling_for(model: &str, context_window: usize) -> usize {
    crate::engine::context::agent_context_char_budget_for(model, context_window)
        / MCP_BUDGET_DIVISOR
}

const MCP_WARNING_PERCENT: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct McpBudget {
    pub request_chars: usize,
    pub mcp_ceiling_chars: usize,
    pub sent_tools: usize,
    pub sent_chars: usize,
    pub sent_tokens: usize,
    pub dropped: Vec<McpDroppedTools>,
    pub warning: Option<String>,
}

impl McpBudget {
    fn new(
        model: &str,
        context_window: usize,
        sent: &[ToolDefinition],
        dropped: Vec<McpDroppedTools>,
    ) -> Self {
        let sent_chars = tool_definitions_chars(sent);
        let mcp_ceiling_chars = mcp_tool_char_ceiling_for(model, context_window);
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
            request_chars: crate::engine::context::agent_context_char_budget_for(
                model,
                context_window,
            ),
            mcp_ceiling_chars,
            sent_tools: sent.len(),
            sent_chars,
            sent_tokens: estimate_tokens_from_chars(sent_chars),
            dropped,
            warning,
        }
    }
}

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

    /// INVARIANT: What the model is told after asking for a start. Both outcomes end in
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStartFailure {
    pub server_id: String,
    pub error: String,
}

fn not_enabled_message(server_id: &str, wire_name: &str) -> String {
    format!(
        "MCP server '{}' is not enabled in this thread, so tool '{}' was NOT run. \
         Call mcp enable with id '{}' first if this task needs it.",
        server_id, wire_name, server_id
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStopOutcome {
    Stopped { name: String },
    WasNotRunning,
}

impl McpManager {
    pub fn new(
        pool: sqlx::PgPool,
        event_bus: crate::engine::event_bus::EventBus,
        workspace_root: std::path::PathBuf,
    ) -> Self {
        Self {
            running: Arc::new(RwLock::new(HashMap::new())),
            tool_surface_generation: Arc::new(AtomicU64::new(0)),
            start_locks: std::sync::Mutex::new(HashMap::new()),
            disabled_tools_writes: Mutex::new(()),
            pool,
            event_bus,
            workspace_root,
        }
    }

    /// INVARIANT: Record that some thread's offered tools may just have changed. Call it
    /// after the change has landed: readers take their stamp before they
    /// read, so a bump after the change can never hide it.
    fn bump_tool_surface(&self) {
        self.tool_surface_generation.fetch_add(1, Ordering::Release);
    }

    async fn take_running(&self, id: &str) -> Option<RunningServer> {
        let mut running = self.running.write().await;
        let removed = running.remove(id);
        if removed.is_some() {
            self.bump_tool_surface();
        }
        removed
    }

    /// INVARIANT: Drop every entry whose process has exited.
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
                log_legacy!(
                    "[MCP] Server '{}' exited on its own, dropping it from the registry",
                    id
                );
            }
        }
        if evicted {
            self.bump_tool_surface();
        }
    }

    pub fn tool_surface_generation(&self) -> u64 {
        self.tool_surface_generation.load(Ordering::Acquire)
    }

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

    /// INVARIANT: Enable `id` in `thread_id`, starting the server first if it is not
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

    /// INVARIANT: Stop a server because someone asked to, which switches it off in every
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
        // INVARIANT: A start already past its enabled-set read finishes first, and is
        // then stopped here, rather than landing after the stop.
        let start_lock = self.start_lock(id);
        let _starting = start_lock.lock().await;
        let outcome = self.stop_server(id).await?;
        self.bump_tool_surface();
        Ok(Some(outcome))
    }

    pub async fn start_server(
        &self,
        id: &str,
    ) -> Result<McpStartOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let server = McpServerStore::get(&self.pool, id)
            .await?
            .ok_or_else(|| format!("MCP server '{}' not found", id))?;
        self.start_loaded(&server).await
    }

    fn start_lock(&self, id: &str) -> Arc<Mutex<()>> {
        Arc::clone(
            self.start_locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(id.to_string())
                .or_default(),
        )
    }

    pub async fn start_loaded(
        &self,
        server: &McpServer,
    ) -> Result<McpStartOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let start_lock = self.start_lock(&server.id);
        let _starting = start_lock.lock().await;
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

    async fn start_server_internal(
        &self,
        server: &McpServer,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        crate::core::mcp_servers::validate_server_id(&server.id)?;
        self.require_launch_approval(server).await?;
        let env = crate::core::mcp_servers::launch_env(&self.pool, server).await?;
        let sandbox =
            crate::runtime::agent_sandbox::Sandbox::resolve(&self.pool, &self.workspace_root)
                .await?;
        let client = McpClient::connect(
            &sandbox,
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
            log_legacy!(
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

    pub fn launch_fingerprint(
        &self,
        server: &McpServer,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let spawn_cwd = std::env::current_dir()
            .map_err(|e| format!("the engine's working directory is unknown: {e}"))?;
        server
            .launch_spec()
            .fingerprint(&spawn_cwd, &self.workspace_root)
    }

    pub async fn approve_launch(
        &self,
        server: &McpServer,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let fingerprint = self.launch_fingerprint(server)?;
        McpServerStore::approve_launch(&self.pool, &self.event_bus, &server.id, &fingerprint, actor)
            .await
    }

    pub async fn is_launch_approved(&self, server: &McpServer) -> Result<bool, sqlx::Error> {
        let Ok(fingerprint) = self.launch_fingerprint(server) else {
            return Ok(false);
        };
        McpServerStore::is_launch_approved(&self.pool, &server.id, &fingerprint).await
    }

    async fn require_launch_approval(
        &self,
        server: &McpServer,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !crate::core::prefs::COMMAND_GUARD.read(&self.pool).await
            || self.is_launch_approved(server).await?
        {
            return Ok(());
        }
        Err(format!(
            "Refused by the command guard: MCP server '{id}' was NOT started. Nobody has \
             approved the command it runs, or the command changed since. To approve it, \
             call the `mcp` tool with action `enable` and id `{id}`, which asks the user first, or \
             have the user press Start in Settings > MCP Servers.",
            id = server.id
        )
        .into())
    }

    /// INVARIANT: Stop a running server.
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

    /// INVARIANT: Remove a server: delete the row, then stop the process.
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

        // INVARIANT: Mirror it onto the running snapshot, which is what the per-request
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

    pub async fn list_servers(
        &self,
    ) -> Result<Vec<McpServerStatus>, Box<dyn std::error::Error + Send + Sync>> {
        self.evict_exited_servers().await;

        let servers = McpServerStore::list(&self.pool).await?;
        let approvals = McpServerStore::launch_approvals(&self.pool).await?;
        let spawn_cwd = std::env::current_dir().ok();
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
                let spec = server.launch_spec();
                let launch_approved = spawn_cwd.as_deref().is_some_and(|cwd| {
                    spec.fingerprint(cwd, &self.workspace_root)
                        .is_ok_and(|fingerprint| {
                            approvals.contains(&(server.id.clone(), fingerprint))
                        })
                });
                McpServerStatus {
                    launch_line: spawn_cwd.as_deref().map(|cwd| {
                        spec.command_line(cwd, &self.workspace_root, LaunchLineMode::Masked)
                    }),
                    launch_approved,
                    ..server_status(&server, tools, tools_source, running_entry.is_some())
                }
            })
            .collect())
    }

    pub async fn set_auto_approve(
        &self,
        id: &str,
        auto_approve: bool,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        McpServerStore::set_auto_approve(&self.pool, &self.event_bus, id, auto_approve, actor)
            .await?;

        let mut running = self.running.write().await;
        if let Some(entry) = running.get_mut(id) {
            entry.server_config.auto_approve = auto_approve;
        }

        let action = if auto_approve { "enabled" } else { "disabled" };
        Ok(format!("Auto-approve {} for MCP server '{}'.", action, id))
    }

    pub async fn not_enabled_refusal(
        &self,
        thread_id: uuid::Uuid,
        server_id: &str,
        wire_name: &str,
    ) -> Option<String> {
        (!self.enabled_servers(thread_id).await.contains(server_id))
            .then(|| not_enabled_message(server_id, wire_name))
    }

    /// INVARIANT: Why this call must not reach the process, or `None` to go ahead.
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

    pub async fn call_tool(
        &self,
        thread_id: uuid::Uuid,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<(String, String, bool), Box<dyn std::error::Error + Send + Sync>> {
        let wire_name = format!("mcp__{}__{}", server_id, tool_name);
        if let Some(refusal) = self
            .not_enabled_refusal(thread_id, server_id, &wire_name)
            .await
        {
            return Err(refusal.into());
        }

        self.evict_exited_servers().await;

        // INVARIANT: Take a handle and let the registry go. Each lookup is its own
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

        // INVARIANT: Dispatch is the gate, not the definition list. Omitting a disabled
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

        let result = {
            let mut client = target.client.lock().await;
            // INVARIANT: The real gate, asked once this call has the server to itself.
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

    /// INVARIANT: The tools of every running server `thread_id` enabled, namespaced as
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
            log_legacy!(
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

    pub async fn budget(
        &self,
        model: &str,
        context_window: usize,
    ) -> Result<McpBudget, Box<dyn std::error::Error + Send + Sync>> {
        let (sent, dropped) = fit_to_ceiling(
            self.offers_if_enabled().await?,
            mcp_tool_char_ceiling_for(model, context_window),
        );
        Ok(McpBudget::new(model, context_window, &sent, dropped))
    }

    pub async fn server_lines(
        &self,
        enabled: &BTreeSet<String>,
        failures: &[McpStartFailure],
        trigger_listing: Option<&[String]>,
    ) -> Vec<String> {
        let servers = match self.list_servers().await {
            Ok(servers) => servers,
            Err(e) => {
                log_legacy!(
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
                log_legacy!(
                    "[MCP] Failed to read the enabled servers of thread {}: {}",
                    thread_id,
                    e
                );
                BTreeSet::new()
            }
        }
    }

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
                log_legacy!(
                    "[MCP] Thread {} did not inherit '{}' from its parent {}: {}",
                    child,
                    server_id,
                    parent,
                    e
                );
            }
        }
    }

    pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
        crate::llm::tool_names::parse_mcp_tool_name(name)
    }
}

struct ToolOffer<'a> {
    tool: &'a McpTool,
    wire_name: Option<String>,
    disabled: bool,
    definition: Option<ToolDefinition>,
}

impl ToolOffer<'_> {
    fn into_offered(self) -> Option<ToolDefinition> {
        if self.disabled {
            return None;
        }
        self.definition
    }

    fn chars(&self) -> usize {
        self.definition
            .as_ref()
            .map_or(0, |d| tool_definitions_chars(std::slice::from_ref(d)))
    }
}

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
                    log_legacy!(
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

fn refused_line(status: &McpServerStatus) -> String {
    format!(
        "- {} ({}): {}. This trigger does not list it, so a call to it is refused in this fire.",
        status.id,
        status.name,
        tool_count_phrase(status)
    )
}

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
        launch_line: None,
        launch_approved: false,
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

/// INVARIANT: FNV-1a over the name, as eight hex digits. Deliberately not `DefaultHasher`,
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

/// INVARIANT: The wire name of every tool in `tools`, positionally aligned with it.
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

    // INVARIANT: A base only one tool rewrites to stays bare, which is every tool on a
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

    // INVARIANT: Truncation, or a tag collision, can still land two tools on one name.
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

    #[test]
    fn colliding_names_stay_distinct_and_each_resolves_to_itself() {
        let tools = tools_named(&["a.b", "a_b", "a-b", "a b"]);
        let wire = wire_tool_names("srv", &tools);

        let unique: std::collections::HashSet<&Option<String>> = wire.iter().collect();
        assert_eq!(unique.len(), wire.len(), "wire names must be unique");
        assert_eq!(wire[2].as_deref(), Some("mcp__srv__a-b"));
        for i in [0, 1, 3] {
            let name = wire[i].as_deref().unwrap();
            assert!(name.starts_with("mcp__srv__a_b_"), "{name}");
            assert_ne!(name, "mcp__srv__a_b");
        }
        assert_round_trips("srv", &tools);
    }

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

    #[test]
    fn a_maximal_server_id_never_yields_a_name_that_loses_the_separator() {
        let server_id = "a".repeat(crate::core::mcp_servers::MAX_SERVER_ID_LEN);
        let tools = tools_named(&["one.tool", "two.tool", "three.tool"]);
        let wire = wire_tool_names(&server_id, &tools);

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
        assert!(
            resolve_wire_tool_name("backstage", &tools, "catalog.get-catalog-entity").is_none()
        );
    }

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

    #[test]
    fn the_budget_report_agrees_with_the_request_budget_and_warns_near_the_ceiling() {
        let ceiling = mcp_tool_char_ceiling(200_000);
        let quiet = McpBudget::new("", 200_000, &[definition_of(ceiling / 2)], Vec::new());
        assert_eq!(
            quiet.request_chars,
            crate::engine::context::agent_context_char_budget(200_000)
        );
        assert_eq!(quiet.mcp_ceiling_chars, ceiling);
        assert_eq!((quiet.sent_tools, quiet.sent_chars), (1, ceiling / 2));
        assert_eq!(quiet.sent_tokens, estimate_tokens_from_chars(ceiling / 2));
        assert_eq!(quiet.warning, None);

        let near = McpBudget::new("", 200_000, &[definition_of(ceiling / 10 * 9)], Vec::new());
        let warning = near.warning.expect("90% of the ceiling warns");
        assert!(warning.contains("90%"), "{warning}");

        let past = McpBudget::new(
            "",
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

        assert_eq!(McpBudget::new("", 200_000, &[], Vec::new()).warning, None);
    }

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

    #[test]
    fn per_server_cost_equals_tool_definitions_chars_over_the_same_definitions() {
        let tools = priced_tools(&["alpha", "beta", "gamma"]);
        let server = server_row("srv", &[]);
        let status = server_status(&server, &tools, McpToolsSource::Live, true);

        let definitions: Vec<ToolDefinition> = tool_offers("srv", &server.name, &tools, &[])
            .into_iter()
            .filter_map(|o| o.definition)
            .collect();
        assert_eq!(definitions.len(), 3);
        assert_eq!(status.chars, tool_definitions_chars(&definitions));
        assert_eq!(status.tokens, estimate_tokens_from_chars(status.chars));
        assert!(status.chars > 0, "a real schema is not free");

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
        assert_eq!(
            status.tools.iter().map(|t| t.chars).sum::<usize>(),
            status.chars
        );
    }

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

        assert_eq!(status.chars + status.disabled_chars, all_on.chars);
        assert_eq!(status.disabled_chars, all_on.tools[1].chars);
        assert!(status.chars < all_on.chars);

        let offered: Vec<String> = tool_offers("srv", &server.name, &tools, &server.disabled_tools)
            .into_iter()
            .filter_map(|o| o.into_offered().map(|d| d.name))
            .collect();
        assert_eq!(offered, vec!["mcp__srv__alpha".to_string()]);
    }

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

        assert_eq!(unobserved.chars, 0);
        assert_eq!(empty.chars, 0);

        let totals = McpCostTotals::of(&[unobserved, empty]);
        assert_eq!(totals.servers, 2);
        assert_eq!(totals.running_servers, 0);
        assert_eq!(totals.stopped_tokens, 0);
    }

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
            crate::runtime::stub_executable::write_stub_executable(&script, STUB_SERVER);
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

        fn registered_env(&self) -> HashMap<String, crate::core::McpEnvValue> {
            self.env
                .iter()
                .map(|(var, value)| (var.clone(), crate::core::McpEnvValue::Plain(value.clone())))
                .collect()
        }

        fn spawns_a_grandchild(mut self) -> Self {
            self.env.insert(
                "STUB_GRANDCHILD_PID_FILE".to_string(),
                self.grandchild_pid_file.display().to_string(),
            );
            self
        }

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

        fn slow_to_answer(mut self, seconds: u32) -> Self {
            self.env
                .insert("STUB_CALL_DELAY".to_string(), seconds.to_string());
            self
        }

        fn floods_on_call(mut self, bytes: usize) -> Self {
            self.env
                .insert("STUB_CALL_FLOOD_BYTES".to_string(), bytes.to_string());
            self
        }

        fn fails_tools_list(mut self, message: &str) -> Self {
            self.env
                .insert("STUB_TOOLS_LIST_ERROR".to_string(), message.to_string());
            self
        }

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

        fn call_started(&self) -> bool {
            self.call_started_file.exists()
        }

        async fn await_call_started(&self) {
            for _ in 0..250 {
                if self.call_started() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the stub never received a tools/call");
        }

        async fn await_exit(&self) {
            for _ in 0..150 {
                if !self.is_alive() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the stub outlived the client that spawned it");
        }

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
        let (manager, bus) = manager_with_unapproved(pool, id, stub).await;
        approve_all(&manager).await;
        (manager, bus)
    }

    async fn manager_with_unapproved(
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
        (
            McpManager::new(pool.clone(), bus.clone(), test_workspace_root()),
            bus,
        )
    }

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

    fn test_workspace_root() -> std::path::PathBuf {
        std::path::PathBuf::from("/workspace")
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
        let manager = McpManager::new(pool.clone(), bus, test_workspace_root());
        approve_all(&manager).await;
        manager
    }

    async fn approve_all(manager: &McpManager) {
        for server in McpServerStore::list(&manager.pool).await.unwrap() {
            manager.approve_launch(&server, None).await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_stopped_server_still_reports_the_manifest_it_last_advertised() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;

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

    #[tokio::test]
    async fn a_failed_start_leaves_the_previous_manifest_intact() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;

        manager.start_server("stub").await.unwrap();
        manager.stop_server("stub").await.unwrap();
        let good = McpServerStore::get(&pool, "stub").await.unwrap().unwrap();
        assert_eq!(good.tools.len(), 1);

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

        let budget = manager.budget("", 0).await.unwrap();
        assert_eq!(budget.mcp_ceiling_chars, 0);
        assert_eq!((budget.sent_tools, budget.sent_chars), (0, 0));
        assert_eq!(budget.dropped[0].tools.len(), 3);
        assert!(budget.warning.is_some());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn gemini_four_budget_report_matches_the_live_tool_surface() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();
        for (model, cap) in [
            ("gemini-4-argon", 32_768),
            ("gemini-4-carbon", 32_768),
            ("gemini-4-neon", 65_536),
        ] {
            let report = manager.budget(model, 1_048_576).await.unwrap();
            let surface = manager
                .tool_surface(thread, mcp_tool_char_ceiling_for(model, 1_048_576))
                .await;
            assert_eq!(report.sent_chars, tool_definitions_chars(&surface.tools));
            assert_eq!(report.request_chars, (1_048_576 - cap) * 3 / 2);
            assert_eq!(
                report.mcp_ceiling_chars,
                report.request_chars / MCP_BUDGET_DIVISOR
            );
        }
        manager.stop_server("stub").await.unwrap();
        crate::test_support::teardown_test_db(&db_name).await;
    }

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

        let budget = manager.budget("", 200_000).await.unwrap();
        assert_eq!(budget.sent_tools, 3, "up's one tool and down's two");
        assert!(budget.dropped.is_empty());

        crate::test_support::teardown_test_db(&db_name).await;
    }

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

        let (result, server_name, _auto_approve) = manager
            .call_tool(thread, "stub", "alpha", serde_json::json!({}))
            .await
            .expect("an enabled tool still dispatches");
        assert_eq!(result, "stub ran");
        assert_eq!(server_name, "Stub");

        crate::test_support::teardown_test_db(&db_name).await;
    }

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

        assert!(!manager.remove_server("stub", None).await.unwrap());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn every_way_a_server_comes_up_tells_the_model_its_tools_are_live_now() {
        assert!(McpStartOutcome::Started { tool_count: 15 }
            .describe("slack")
            .contains(CALLABLE_NOW));
        assert!(McpStartOutcome::AlreadyRunning { tool_count: 15 }
            .describe("slack")
            .contains(CALLABLE_NOW));

        let (pool, db_name) = crate::test_support::setup_test_db().await;
        crate::core::preferences::PreferenceStore::set_row_for_test(
            &pool,
            crate::core::prefs::COMMAND_GUARD.key(),
            "false",
        )
        .await
        .unwrap();
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

    #[tokio::test]
    async fn a_server_whose_process_died_is_evicted_before_anything_trusts_it() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        let thread = thread_enabling(&manager, &["stub"]).await;
        manager.start_server("stub").await.unwrap();
        let live = manager.tool_surface(thread, usize::MAX).await;
        assert_eq!(live.tools.len(), 2);

        stub.kill_and_await_stop().await;
        assert_eq!(
            manager.start_server("stub").await.unwrap(),
            McpStartOutcome::Started { tool_count: 2 },
            "bringing a dead server back is a start, never AlreadyRunning"
        );

        stub.kill_and_await_stop().await;
        let (result, _name, _auto_approve) = manager
            .call_tool(thread, "stub", "alpha", serde_json::json!({}))
            .await
            .expect("a call on a dead server restarts it");
        assert_eq!(result, "stub ran");

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

        manager.stop_server("stub").await.unwrap();
        assert!(manager.remove_server("stub", None).await.unwrap());
        assert_eq!(manager.tool_surface_generation(), stopped.generation);

        crate::test_support::teardown_test_db(&db_name).await;
    }

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
        before_restart.stop_server("stub").await.unwrap();
        drop(before_restart);
        assert_eq!(events_named(&pool, "McpServerStopped").await, 0);

        let after_restart = McpManager::new(pool.clone(), bus.clone(), test_workspace_root());
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

        let unlisted = manager
            .server_lines(&BTreeSet::new(), &[], Some(&["other".to_string()]))
            .await;
        assert_eq!(
            unlisted,
            ["- stub (Stub): 1 tool. This trigger does not list it, so a call to it is refused in this fire."]
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

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

    #[tokio::test]
    async fn with_the_guard_on_an_unapproved_launch_line_never_spawns() {
        use crate::core::preferences::PreferenceStore;
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with_unapproved(&pool, "stub", &stub).await;
        let thread_id = thread_enabling(&manager, &["stub"]).await;
        PreferenceStore::set_row_for_test(&pool, crate::core::prefs::COMMAND_GUARD.key(), "true")
            .await
            .unwrap();

        let refused = manager.start_server("stub").await.unwrap_err().to_string();
        assert!(refused.contains("NOT started"), "{refused}");
        let failures = manager.start_enabled_servers(thread_id).await;
        assert_eq!(failures.len(), 1);
        assert!(failures[0].error.contains("Settings > MCP Servers"));
        assert!(
            !stub.pid_file.exists(),
            "a refused start spawned the server"
        );

        let server = McpServerStore::get(&pool, "stub").await.unwrap().unwrap();
        manager.approve_launch(&server, None).await.unwrap();
        assert!(manager.start_enabled_servers(thread_id).await.is_empty());
        assert!(stub.pid_file.exists());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn with_the_guard_off_an_unapproved_launch_line_starts() {
        use crate::core::preferences::PreferenceStore;
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        PreferenceStore::set_row_for_test(&pool, crate::core::prefs::COMMAND_GUARD.key(), "false")
            .await
            .unwrap();
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with_unapproved(&pool, "stub", &stub).await;
        let server = McpServerStore::get(&pool, "stub").await.unwrap().unwrap();
        assert!(!manager.is_launch_approved(&server).await.unwrap());

        assert_eq!(
            manager.start_server("stub").await.unwrap(),
            McpStartOutcome::Started { tool_count: 1 }
        );
        assert!(
            !manager.is_launch_approved(&server).await.unwrap(),
            "a start with the guard off approves nothing"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn the_server_list_reports_the_launch_line_and_its_approval() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with_unapproved(&pool, "stub", &stub).await;
        let server = McpServerStore::get(&pool, "stub").await.unwrap().unwrap();
        let before = manager.list_servers().await.unwrap().remove(0);
        assert!(!before.launch_approved);
        let line = before.launch_line.expect("the working directory is known");
        assert!(line.contains(&server.command), "{line}");

        manager.approve_launch(&server, None).await.unwrap();
        assert!(manager.list_servers().await.unwrap()[0].launch_approved);

        crate::test_support::teardown_test_db(&db_name).await;
    }

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

        broken.await_exit().await;

        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(!status.running);
        assert_eq!(status.tools_source, McpToolsSource::NeverObserved);
        assert!(status.tools.is_empty());
        assert!(manager
            .tool_surface(thread, usize::MAX)
            .await
            .tools
            .is_empty());

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
        approve_all(&manager).await;
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

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_a_client_stops_the_processes_its_server_started() {
        let stub = StubServer::new(&[]).spawns_a_grandchild();
        let client = McpClient::connect(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            "stub",
            &stub.command,
            &[],
            &stub.env,
            ActorMode::Agent,
        )
        .await
        .unwrap();

        drop(client);

        stub.await_exit().await;
        stub.await_grandchild_stop().await;
    }

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
