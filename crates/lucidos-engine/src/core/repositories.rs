use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Repository {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct RepositoryStore;

impl RepositoryStore {
    pub async fn list(pool: &PgPool) -> Result<Vec<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, created_at FROM repositories ORDER BY name",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn add(
        pool: &PgPool,
        name: &str,
        path: &str,
        description: Option<&str>,
    ) -> Result<Repository, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "INSERT INTO repositories (name, path, description) VALUES ($1, $2, $3) \
             ON CONFLICT (path) DO UPDATE SET name = EXCLUDED.name, description = EXCLUDED.description \
             RETURNING id, name, path, description, created_at",
        )
        .bind(name)
        .bind(path)
        .bind(description)
        .fetch_one(pool)
        .await
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, created_at FROM repositories WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn get_by_name(pool: &PgPool, name: &str) -> Result<Option<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, created_at FROM repositories WHERE LOWER(name) = LOWER($1)",
        )
        .bind(name)
        .fetch_optional(pool)
        .await
    }

    /// Resolve a repository by UUID or case-insensitive name.
    pub async fn resolve(
        pool: &PgPool,
        id_or_name: &str,
    ) -> Result<Option<Repository>, sqlx::Error> {
        if let Ok(uuid) = Uuid::parse_str(id_or_name) {
            let repo = Self::get(pool, uuid).await?;
            if repo.is_some() {
                return Ok(repo);
            }
        }
        Self::get_by_name(pool, id_or_name).await
    }

    /// Idempotent upsert — inserts a repository, or updates the name if one already exists at the path.
    pub async fn ensure_exists(pool: &PgPool, name: &str, path: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO repositories (name, path) VALUES ($1, $2) ON CONFLICT (path) DO UPDATE SET name = EXCLUDED.name",
        )
        .bind(name)
        .bind(path)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn remove(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM repositories WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
