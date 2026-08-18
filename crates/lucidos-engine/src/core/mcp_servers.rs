use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub auto_approve: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// The tool manifest as last observed, verbatim. Only a running server
    /// advertises tools, so this is what answers "what does this cost" for one
    /// that is switched off.
    pub tools: Vec<crate::mcp::types::McpTool>,
    /// When [`Self::tools`] was last read off a live server. `None` means never
    /// observed, which is NOT the same as observed-and-empty: one is unknown
    /// and the other is genuinely free.
    pub tools_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Tools the user switched off, by WIRE name. Filtered out of the
    /// definitions every request carries.
    pub disabled_tools: Vec<String>,
}

/// The columns every read selects, in the order [`row_to_server`] unpacks them.
const SERVER_COLUMNS: &str = "id, name, command, args, env, auto_approve, created_at, \
                              tools, tools_observed_at, disabled_tools";

/// Raw DB row from mcp_servers, in [`SERVER_COLUMNS`] order.
type McpServerRow = (
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    bool,
    chrono::DateTime<chrono::Utc>,
    serde_json::Value,
    Option<chrono::DateTime<chrono::Utc>>,
    Vec<String>,
);

/// Longest server id that still leaves room for `mcp__<id>__<tool>` with one
/// character of tool name. Past it the wire name is truncated through the
/// separator, and no tool call parses back to a server.
pub(crate) const MAX_SERVER_ID_LEN: usize =
    crate::llm::validate::MAX_TOOL_NAME_LEN - "mcp__".len() - "__".len() - 1;

/// Reject a server id the tool-name layer could not carry.
///
/// The id is half of every wire tool name (`mcp__<id>__<tool>`), so it has to
/// survive that round trip untouched. `__` is the separator itself. Any other
/// character outside the Messages API alphabet would be rewritten, leaving a
/// server whose tools are visible but cannot be dispatched.
pub(crate) fn validate_server_id(id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let shape = "use letters, digits, '-' and single '_' (e.g. 'backstage', 'dev-docs')";
    if !crate::llm::validate::is_wire_safe_tool_name(id) {
        return Err(format!("MCP server id '{}' is not usable: {}", id, shape).into());
    }
    if id.contains("__") {
        return Err(format!(
            "MCP server id '{}' must not contain '__', which separates the id from the tool name: {}",
            id, shape
        )
        .into());
    }
    if id.len() > MAX_SERVER_ID_LEN {
        return Err(format!(
            "MCP server id '{}' is {} characters, over the {} that fit in a tool name: {}",
            id,
            id.len(),
            MAX_SERVER_ID_LEN,
            shape
        )
        .into());
    }
    Ok(())
}

/// The stored form of a disabled-tool selection: sorted and deduped, so the
/// column holds a set. A client resending the same names in another order is
/// then not a change, and nothing re-announces.
fn normalize_disabled_tools(names: &[String]) -> Vec<String> {
    let mut set: Vec<String> = names.to_vec();
    set.sort();
    set.dedup();
    set
}

/// The registry of external MCP servers whose tools the agent can call.
///
/// **No caller can skip the event.** [`Self::register`], [`Self::unregister`],
/// [`Self::set_auto_approve`] and [`Self::set_disabled_tools`] are the reachable
/// mutators that change what the user chose; the raw row writes are private to
/// this module. Each moves the agent's own tool surface or the permission on
/// it, so each belongs on the timeline: before this, adding an MCP server from
/// chat left no trace anywhere and nothing could react to it.
///
/// [`Self::set_tools`] is the one reachable writer that stays silent, and it is
/// registered as such in `core::announced_surfaces`. It caches what a live
/// server reported rather than recording a decision, and the only event that
/// could carry it describes an auto-approve change. `tools_observed_at` is its
/// signal instead.
///
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
pub struct McpServerStore;

fn row_to_server(row: McpServerRow) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
    let (
        id,
        name,
        command,
        args,
        env,
        auto_approve,
        created_at,
        tools,
        tools_observed_at,
        disabled_tools,
    ) = row;
    Ok(McpServer {
        id,
        name,
        command,
        args: serde_json::from_value(args)?,
        env: serde_json::from_value(env)?,
        auto_approve,
        created_at,
        tools: serde_json::from_value(tools)?,
        tools_observed_at,
        disabled_tools,
    })
}

impl McpServerStore {
    pub async fn list(
        pool: &PgPool,
    ) -> Result<Vec<McpServer>, Box<dyn std::error::Error + Send + Sync>> {
        let rows: Vec<McpServerRow> = sqlx::query_as(&format!(
            "SELECT {SERVER_COLUMNS} FROM mcp_servers ORDER BY created_at"
        ))
        .fetch_all(pool)
        .await?;

        rows.into_iter().map(row_to_server).collect()
    }

    pub async fn get(
        pool: &PgPool,
        id: &str,
    ) -> Result<Option<McpServer>, Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<McpServerRow> = sqlx::query_as(&format!(
            "SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(pool)
        .await?;

        row.map(row_to_server).transpose()
    }

    /// Write (or rewrite) a server row. **Private on purpose**:
    /// [`Self::register`] is the reachable mutator, and it emits. See the
    /// type-level doc.
    async fn upsert_row(
        pool: &PgPool,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
        let args_json = serde_json::to_value(args)?;
        let env_json = serde_json::to_value(env)?;

        // RETURNING the whole row, not just what was written: the ON CONFLICT
        // arm leaves auto_approve, the cached manifest and the disabled set
        // alone. A previously auto-approved server must still report `true`, or
        // the caller connects with a flag that disagrees with the DB row.
        let row: McpServerRow = sqlx::query_as(&format!(
            "INSERT INTO mcp_servers (id, name, command, args, env) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, command = EXCLUDED.command, args = EXCLUDED.args, env = EXCLUDED.env \
             RETURNING {SERVER_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(command)
        .bind(&args_json)
        .bind(&env_json)
        .fetch_one(pool)
        .await?;

        row_to_server(row)
    }

    /// Delete a server row. **Private on purpose**: [`Self::unregister`] emits.
    async fn delete_row(
        pool: &PgPool,
        id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Flip auto-approve on a server row. **Private on purpose**:
    /// [`Self::set_auto_approve`] emits.
    ///
    /// Returns `None` when no such server exists, `Some(changed)` otherwise.
    /// Both bits are needed and `rows_affected` gives neither cleanly: Postgres
    /// writes a new tuple version even when the value is identical, so an
    /// affected row means "it exists", never "it changed". The self-join reads
    /// the pre-update value in the same statement (the subselect sees the
    /// snapshot from before the write), so one round trip answers both.
    async fn set_auto_approve_row(
        pool: &PgPool,
        id: &str,
        auto_approve: bool,
    ) -> Result<Option<bool>, Box<dyn std::error::Error + Send + Sync>> {
        let changed: Option<bool> = sqlx::query_scalar(
            "UPDATE mcp_servers AS m SET auto_approve = $2 \
             FROM (SELECT id, auto_approve FROM mcp_servers WHERE id = $1) AS prior \
             WHERE m.id = prior.id \
             RETURNING (prior.auto_approve IS DISTINCT FROM $2)",
        )
        .bind(id)
        .bind(auto_approve)
        .fetch_optional(pool)
        .await?;
        Ok(changed)
    }

    /// Cache the tool manifest a live server just advertised, stamping
    /// `tools_observed_at`. Deliberately silent, see the type-level doc.
    ///
    /// Returns whether the row existed. A server removed while its process was
    /// connecting writes nothing rather than resurrecting the row, which an
    /// UPSERT here would do.
    pub async fn set_tools(
        pool: &PgPool,
        id: &str,
        tools: &[crate::mcp::types::McpTool],
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let tools_json = serde_json::to_value(tools)?;
        let result = sqlx::query(
            "UPDATE mcp_servers SET tools = $2, tools_observed_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .bind(&tools_json)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Replace a server's disabled-tool set. **Private on purpose**:
    /// [`Self::set_disabled_tools`] emits.
    ///
    /// Same self-join shape as [`Self::set_auto_approve_row`], and for the same
    /// reason: `rows_affected` answers "does it exist", never "did it change".
    /// Returns `None` when no such server exists, `Some(changed)` otherwise.
    async fn set_disabled_tools_row(
        pool: &PgPool,
        id: &str,
        disabled_tools: &[String],
    ) -> Result<Option<bool>, Box<dyn std::error::Error + Send + Sync>> {
        let changed: Option<bool> = sqlx::query_scalar(
            "UPDATE mcp_servers AS m SET disabled_tools = $2 \
             FROM (SELECT id, disabled_tools FROM mcp_servers WHERE id = $1) AS prior \
             WHERE m.id = prior.id \
             RETURNING (prior.disabled_tools IS DISTINCT FROM $2)",
        )
        .bind(id)
        .bind(disabled_tools)
        .fetch_optional(pool)
        .await?;
        Ok(changed)
    }

    /// Set which of a server's tools are switched off, by wire name, and
    /// announce it. Disabling a tool takes it out of every request, so it
    /// changes the agent's own tool surface exactly as a registration does.
    ///
    /// Announces only when the set actually MOVED, the same rule
    /// [`Self::set_auto_approve`] follows. Re-saving the current selection is
    /// not a change and must not re-fire subscribers.
    ///
    /// Returns the stored set, or `None` when no such server exists.
    pub async fn set_disabled_tools(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        disabled_tools: &[String],
        actor: Option<MessageOrigin>,
    ) -> Result<Option<Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
        let wanted = normalize_disabled_tools(disabled_tools);
        let Some(changed) = Self::set_disabled_tools_row(pool, id, &wanted).await? else {
            return Ok(None);
        };
        if changed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::McpServerDisabledToolsChanged {
                        server_id: id.to_string(),
                        disabled_tools: wanted.clone(),
                        actor,
                    }),
                    "[MCP] McpServerDisabledToolsChanged",
                )
                .await;
        }
        Ok(Some(wanted))
    }

    /// Register (or re-register) a server and announce it. The only way to add
    /// one.
    ///
    /// Announces unconditionally, including on a re-registration: the row is
    /// rewritten with a new command / args / env, which changes what the tools
    /// actually do even when the id is unchanged.
    #[allow(clippy::too_many_arguments)] // one arg per server column, plus the bus and actor
    pub async fn register(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        actor: Option<MessageOrigin>,
    ) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
        validate_server_id(id)?;
        let server = Self::upsert_row(pool, id, name, command, args, env).await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::McpServerRegistered {
                    server_id: server.id.clone(),
                    name: server.name.clone(),
                    actor,
                }),
                "[MCP] McpServerRegistered",
            )
            .await;
        Ok(server)
    }

    /// Unregister a server and announce it. Announces only when a row was
    /// actually removed.
    pub async fn unregister(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let removed = Self::delete_row(pool, id).await?;
        if removed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::McpServerRemoved {
                        server_id: id.to_string(),
                        actor,
                    }),
                    "[MCP] McpServerRemoved",
                )
                .await;
        }
        Ok(removed)
    }

    /// Set a server's auto-approve flag and announce it. This decides whether
    /// its tool calls prompt the user, so it is a permission change, not a
    /// preference.
    ///
    /// Returns whether the server exists (what callers report to the user), but
    /// announces only when the flag actually MOVED. Re-asserting the current
    /// value would otherwise put a permission-change entry on the timeline for
    /// a permission that did not change, and re-fire every
    /// `on_event: McpServerUpdated` trigger.
    pub async fn set_auto_approve(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        auto_approve: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let outcome = Self::set_auto_approve_row(pool, id, auto_approve).await?;
        if outcome == Some(true) {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::McpServerUpdated {
                        server_id: id.to_string(),
                        auto_approve,
                        actor,
                    }),
                    "[MCP] McpServerUpdated",
                )
                .await;
        }
        Ok(outcome.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An id that cannot round-trip through `mcp__<id>__<tool>` is refused
    /// where the cause is obvious. The alternative is a server whose tools the
    /// agent can see but never call.
    #[test]
    fn server_id_must_survive_the_wire_tool_name() {
        for ok in ["backstage", "dev-docs", "roblox_studio", "bq2"] {
            assert!(validate_server_id(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in ["", "back.stage", "my server", "a__b", "café"] {
            assert!(
                validate_server_id(bad).is_err(),
                "{bad:?} should be refused"
            );
        }
    }

    /// An id can be wire-safe on its own and still leave no room for the
    /// separator plus a tool name. The composed name is then truncated through
    /// the `__`, and every tool the server advertises fails to parse back.
    #[test]
    fn the_longest_accepted_server_id_still_leaves_room_for_a_tool() {
        let longest = "a".repeat(MAX_SERVER_ID_LEN);
        assert!(validate_server_id(&longest).is_ok());
        assert!(validate_server_id(&"a".repeat(MAX_SERVER_ID_LEN + 1)).is_err());

        // The tightest wire name that id can produce still round-trips.
        let wire = crate::llm::validate::wire_safe_tool_name(&format!("mcp__{longest}__x"));
        assert!(crate::llm::validate::is_wire_safe_tool_name(&wire));
        assert_eq!(
            crate::mcp::McpManager::parse_mcp_tool_name(&wire),
            Some((longest, "x".to_string()))
        );
    }

    /// Registering an MCP server changes the agent's own tool surface, and
    /// auto-approve decides whether those tools prompt. Both now reach the
    /// timeline; before this they were written silently, so asking the agent in
    /// chat to add a server left no trace anywhere.
    #[tokio::test]
    async fn every_registry_mutation_announces() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "McpServerRegistered").await, 1);

        // A re-registration rewrites command / args / env, so what the tools do
        // changed even though the id did not.
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd2", &[], &env, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "McpServerRegistered").await, 2);

        assert!(
            McpServerStore::set_auto_approve(&pool, &bus, "srv", true, None)
                .await
                .unwrap()
        );
        assert_eq!(emitted(&pool, "McpServerUpdated").await, 1);

        // Re-asserting the value the flag already holds still reports that the
        // server exists, but must not announce: a Postgres UPDATE writes a new
        // tuple version regardless of value equality, so rows_affected would
        // say "changed" for a permission that did not change.
        assert!(
            McpServerStore::set_auto_approve(&pool, &bus, "srv", true, None)
                .await
                .unwrap(),
            "the server still exists"
        );
        assert_eq!(
            emitted(&pool, "McpServerUpdated").await,
            1,
            "a no-op toggle must not announce a permission change"
        );

        assert!(
            !McpServerStore::set_auto_approve(&pool, &bus, "missing", true, None)
                .await
                .unwrap()
        );
        assert_eq!(
            emitted(&pool, "McpServerUpdated").await,
            1,
            "a toggle that matched no row must not announce"
        );

        assert!(McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap());
        assert_eq!(emitted(&pool, "McpServerRemoved").await, 1);
        assert!(!McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap());
        assert_eq!(
            emitted(&pool, "McpServerRemoved").await,
            1,
            "second unregister removes nothing and therefore announces nothing"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    fn tool(name: &str) -> crate::mcp::types::McpTool {
        crate::mcp::types::McpTool {
            name: name.to_string(),
            description: Some(format!("does {name}")),
            input_schema: Some(serde_json::json!({"type": "object", "properties": {}})),
        }
    }

    /// The manifest has to survive the process that reported it. Reading cost
    /// back for a switched-off server is the whole reason the column exists.
    #[tokio::test]
    async fn the_tool_manifest_round_trips_and_never_observed_is_not_empty() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();

        // A fresh row is "never observed", which is NOT an empty manifest: the
        // engine does not know what this server offers.
        let fresh = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert!(fresh.tools.is_empty());
        assert!(
            fresh.tools_observed_at.is_none(),
            "nothing has connected yet, so there is no observation to stamp"
        );

        let observed = [tool("alpha"), tool("beta")];
        assert!(McpServerStore::set_tools(&pool, "srv", &observed)
            .await
            .unwrap());

        let cached = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert_eq!(
            cached.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(cached.tools[0].description.as_deref(), Some("does alpha"));
        assert!(
            cached.tools[0].input_schema.is_some(),
            "the schema is most of a definition's cost, so it has to survive"
        );
        assert!(cached.tools_observed_at.is_some());

        // Observed-and-empty is a third state, distinct from never-observed:
        // this server really does cost nothing.
        assert!(McpServerStore::set_tools(&pool, "srv", &[]).await.unwrap());
        let emptied = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert!(emptied.tools.is_empty());
        assert!(emptied.tools_observed_at.is_some());

        // A row that is gone writes nothing rather than resurrecting itself.
        assert!(!McpServerStore::set_tools(&pool, "missing", &observed)
            .await
            .unwrap());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Re-registering a server must not drop the manifest or the disabled set:
    /// the ON CONFLICT arm leaves both alone, so the returned struct has to
    /// report them rather than defaulting.
    #[tokio::test]
    async fn reregistering_keeps_the_manifest_and_the_disabled_set() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();
        McpServerStore::set_tools(&pool, "srv", &[tool("alpha")])
            .await
            .unwrap();
        McpServerStore::set_disabled_tools(
            &pool,
            &bus,
            "srv",
            &["mcp__srv__alpha".to_string()],
            None,
        )
        .await
        .unwrap();

        let again = McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd2", &[], &env, None)
            .await
            .unwrap();
        assert_eq!(again.tools.len(), 1, "the manifest survives a re-register");
        assert!(again.tools_observed_at.is_some());
        assert_eq!(again.disabled_tools, vec!["mcp__srv__alpha".to_string()]);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Switching a tool off narrows the agent's own tool surface, so it goes on
    /// the timeline. Caching a manifest does not: that is an observation, and
    /// the only candidate event describes an auto-approve change.
    #[tokio::test]
    async fn disabling_a_tool_announces_but_caching_a_manifest_does_not() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();
        McpServerStore::set_tools(&pool, "srv", &[tool("alpha")])
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "McpServerUpdated").await, 0);
        assert_eq!(emitted(&pool, "McpServerDisabledToolsChanged").await, 0);

        let disabled = vec!["mcp__srv__beta".to_string(), "mcp__srv__alpha".to_string()];
        let stored = McpServerStore::set_disabled_tools(&pool, &bus, "srv", &disabled, None)
            .await
            .unwrap()
            .expect("the server exists");
        assert_eq!(
            stored,
            vec!["mcp__srv__alpha".to_string(), "mcp__srv__beta".to_string()],
            "stored as a set, so a reordered save is not a change"
        );
        assert_eq!(emitted(&pool, "McpServerDisabledToolsChanged").await, 1);

        // Re-sending the same selection in another order still reports the
        // server exists, and must not announce.
        let resent = vec!["mcp__srv__alpha".to_string(), "mcp__srv__beta".to_string()];
        assert!(
            McpServerStore::set_disabled_tools(&pool, &bus, "srv", &resent, None)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            emitted(&pool, "McpServerDisabledToolsChanged").await,
            1,
            "re-saving the current selection changed nothing"
        );

        // Clearing it IS a change: the tools come back.
        assert!(
            McpServerStore::set_disabled_tools(&pool, &bus, "srv", &[], None)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(emitted(&pool, "McpServerDisabledToolsChanged").await, 2);

        assert!(
            McpServerStore::set_disabled_tools(&pool, &bus, "missing", &disabled, None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            emitted(&pool, "McpServerDisabledToolsChanged").await,
            2,
            "a write that matched no row must not announce"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn reinsert_preserves_auto_approve() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        let args = vec!["--flag".to_string()];
        let env = HashMap::new();

        // A brand-new server defaults to auto_approve = false.
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let created = McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &args, &env, None)
            .await
            .unwrap();
        assert!(
            !created.auto_approve,
            "new server should default to auto_approve = false"
        );

        // User opts the server into auto-approve.
        assert!(
            McpServerStore::set_auto_approve(&pool, &bus, "srv", true, None)
                .await
                .unwrap()
        );

        // Re-registering the same id hits ON CONFLICT DO UPDATE, which does NOT
        // touch auto_approve. The returned struct must reflect the real DB value
        // (true), not a hardcoded false — this is the regression guard for the
        // bug where a re-register silently disabled auto-approve in memory.
        let reinserted =
            McpServerStore::register(&pool, &bus, "srv", "Srv Renamed", "cmd2", &args, &env, None)
                .await
                .unwrap();
        assert!(
            reinserted.auto_approve,
            "re-registering an auto-approved server must keep auto_approve = true"
        );
        assert_eq!(
            reinserted.name, "Srv Renamed",
            "ON CONFLICT should update the mutable fields"
        );

        // The DB row agrees — no in-memory/DB divergence.
        let fetched = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert!(fetched.auto_approve);

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
