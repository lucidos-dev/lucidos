use serde::{Deserialize, Serialize};
use sqlx::PgPool;

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

pub struct McpServerStore;

fn row_to_server(
    row: McpServerRow,
) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
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

    pub async fn insert(
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

    pub async fn delete(
        pool: &PgPool,
        id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn set_auto_approve(
        pool: &PgPool,
        id: &str,
        auto_approve: bool,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("UPDATE mcp_servers SET auto_approve = $2 WHERE id = $1")
            .bind(id)
            .bind(auto_approve)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn reinsert_preserves_auto_approve() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        let args = vec!["--flag".to_string()];
        let env = HashMap::new();

        // A brand-new server defaults to auto_approve = false.
        let created = McpServerStore::insert(&pool, "srv", "Srv", "cmd", &args, &env)
            .await
            .unwrap();
        assert!(
            !created.auto_approve,
            "new server should default to auto_approve = false"
        );

        // User opts the server into auto-approve.
        assert!(McpServerStore::set_auto_approve(&pool, "srv", true)
            .await
            .unwrap());

        // Re-registering the same id hits ON CONFLICT DO UPDATE, which does NOT
        // touch auto_approve. The returned struct must reflect the real DB value
        // (true), not a hardcoded false — this is the regression guard for the
        // bug where a re-register silently disabled auto-approve in memory.
        let reinserted =
            McpServerStore::insert(&pool, "srv", "Srv Renamed", "cmd2", &args, &env)
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
