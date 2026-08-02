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
}

/// Raw DB row from mcp_servers: (id, name, command, args, env, auto_approve, created_at).
type McpServerRow = (
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    bool,
    chrono::DateTime<chrono::Utc>,
);

/// The registry of external MCP servers whose tools the agent can call.
///
/// **No caller can skip the event.** [`Self::register`], [`Self::unregister`]
/// and [`Self::set_auto_approve`] are the only reachable mutators; the raw row
/// writes are private to this module. Registering a server changes the agent's
/// own tool surface, and auto-approve decides whether those tools prompt, so
/// both belong on the timeline: before this, adding an MCP server from chat
/// left no trace anywhere and nothing could react to it.
///
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
pub struct McpServerStore;

fn row_to_server(row: McpServerRow) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
    let (id, name, command, args, env, auto_approve, created_at) = row;
    Ok(McpServer {
        id,
        name,
        command,
        args: serde_json::from_value(args)?,
        env: serde_json::from_value(env)?,
        auto_approve,
        created_at,
    })
}

impl McpServerStore {
    pub async fn list(
        pool: &PgPool,
    ) -> Result<Vec<McpServer>, Box<dyn std::error::Error + Send + Sync>> {
        let rows: Vec<McpServerRow> =
            sqlx::query_as(
                "SELECT id, name, command, args, env, auto_approve, created_at FROM mcp_servers ORDER BY created_at"
            )
            .fetch_all(pool)
            .await?;

        rows.into_iter().map(row_to_server).collect()
    }

    pub async fn get(
        pool: &PgPool,
        id: &str,
    ) -> Result<Option<McpServer>, Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<McpServerRow> =
            sqlx::query_as(
                "SELECT id, name, command, args, env, auto_approve, created_at FROM mcp_servers WHERE id = $1"
            )
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

        // RETURNING auto_approve (not a hardcoded `false`): on an ON CONFLICT
        // update of an existing server, the UPDATE leaves auto_approve untouched,
        // so a server that was previously auto-approved must keep reporting
        // `true` in the returned struct — otherwise the caller connects with a
        // stale flag that disagrees with the DB row.
        let row: (chrono::DateTime<chrono::Utc>, bool) = sqlx::query_as(
            "INSERT INTO mcp_servers (id, name, command, args, env) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, command = EXCLUDED.command, args = EXCLUDED.args, env = EXCLUDED.env \
             RETURNING created_at, auto_approve"
        )
        .bind(id)
        .bind(name)
        .bind(command)
        .bind(&args_json)
        .bind(&env_json)
        .fetch_one(pool)
        .await?;

        Ok(McpServer {
            id: id.to_string(),
            name: name.to_string(),
            command: command.to_string(),
            args: args.to_vec(),
            env: env.clone(),
            auto_approve: row.1,
            created_at: row.0,
        })
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
