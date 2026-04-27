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

        Ok(rows
            .into_iter()
            .map(
                |(id, name, command, args, env, auto_approve, created_at)| McpServer {
                    id,
                    name,
                    command,
                    args: serde_json::from_value(args).unwrap_or_default(),
                    env: serde_json::from_value(env).unwrap_or_default(),
                    auto_approve,
                    created_at,
                },
            )
            .collect())
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

        Ok(row.map(
            |(id, name, command, args, env, auto_approve, created_at)| McpServer {
                id,
                name,
                command,
                args: serde_json::from_value(args).unwrap_or_default(),
                env: serde_json::from_value(env).unwrap_or_default(),
                auto_approve,
                created_at,
            },
        ))
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

        let row: (chrono::DateTime<chrono::Utc>,) = sqlx::query_as(
            "INSERT INTO mcp_servers (id, name, command, args, env) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, command = EXCLUDED.command, args = EXCLUDED.args, env = EXCLUDED.env \
             RETURNING created_at"
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
            auto_approve: false,
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
