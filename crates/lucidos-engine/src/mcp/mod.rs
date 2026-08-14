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
    /// Registry mutations announce through here. Held rather than passed per
    /// call because `McpServerStore`'s mutators require it: registering a
    /// server changes the agent's tool surface, and that is not the caller's
    /// choice to skip (see `core::announced_surfaces`).
    event_bus: crate::engine::event_bus::EventBus,
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
    pub fn new(pool: sqlx::PgPool, event_bus: crate::engine::event_bus::EventBus) -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
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
    ///
    /// Validates the id here and not only at registration, because a stored row
    /// is not proof that it passed. Only a running server advertises tools, so
    /// this is the gate that keeps every advertised tool dispatchable. Letting
    /// an unusable id run gives the model tools no call can reach.
    async fn start_server_internal(
        &self,
        server: &McpServer,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        crate::core::mcp_servers::validate_server_id(&server.id)?;
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

        if McpServerStore::unregister(&self.pool, &self.event_bus, id, None).await? {
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
            let running_entry = running.get(&server.id);
            let (tool_count, tools) = if let Some(entry) = running_entry {
                let names: Vec<String> =
                    entry.client.tools.iter().map(|t| t.name.clone()).collect();
                (names.len(), names)
            } else {
                (0, Vec::new())
            };

            statuses.push(McpServerStatus {
                id: server.id,
                name: server.name,
                running: running_entry.is_some(),
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
        McpServerStore::set_auto_approve(&self.pool, &self.event_bus, id, auto_approve, None)
            .await?;

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

        // `tool_name` is the wire name the model was shown, which is not always
        // what the server calls the tool. Resolve before dispatching.
        let target = resolve_wire_tool_name(server_id, &entry.client.tools, tool_name)
            .map(|t| t.name.clone())
            .ok_or_else(|| {
                let available: Vec<String> = wire_tool_names(server_id, &entry.client.tools)
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

        let result = entry.client.call_tool(&target, arguments).await?;

        Ok((result, server_name, auto_approve))
    }

    /// Get tool definitions for all running servers, namespaced as mcp__{server_id}__{tool_name}.
    pub async fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        let running = self.running.lock().await;
        let mut tools = Vec::new();

        for (server_id, entry) in running.iter() {
            let wire_names = wire_tool_names(server_id, &entry.client.tools);
            for (wire_name, mcp_tool) in wire_names.into_iter().zip(&entry.client.tools) {
                let Some(wire_name) = wire_name else {
                    log!(
                        "[MCP] Tool '{}' on server '{}' has no name that fits the tool-name limit, not offering it",
                        mcp_tool.name,
                        server_id
                    );
                    continue;
                };
                tools.push(mcp_tool_to_definition(
                    wire_name,
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
}
