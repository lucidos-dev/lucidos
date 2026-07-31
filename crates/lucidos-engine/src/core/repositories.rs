use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Fixed namespace for deriving deterministic repository ids via UUIDv5.
/// Generated once and frozen: changing it would re-derive every repo's id and
/// re-orphan every `thread_summaries.cc_repo_id` binding — exactly the failure
/// this scheme exists to prevent.
const REPO_ID_NAMESPACE: Uuid = Uuid::from_u128(0xb6c1f4e2_5a3d_4f8b_9c2e_1d7a0f3e6b54);

/// A repository's **deterministic identity**. Derived from the repo's git
/// root-commit SHA (intrinsic to the history, so it survives moving, renaming,
/// re-cloning, and a registry wipe — every re-registration recomputes the same
/// id from disk), falling back to the canonical path for a repo with no commits
/// yet. Replaces the former random `gen_random_uuid()` surrogate PK, whose
/// regeneration orphaned coding-agent threads. See `docs/glossary.md`
/// § "deterministic repo id".
pub fn deterministic_id(root_commit_sha: Option<&str>, canonical_path: &str) -> Uuid {
    match root_commit_sha {
        Some(sha) if !sha.trim().is_empty() => {
            Uuid::new_v5(&REPO_ID_NAMESPACE, sha.trim().as_bytes())
        }
        _ => Uuid::new_v5(&REPO_ID_NAMESPACE, canonical_path.as_bytes()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Repository {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    /// The repo's root-commit SHA — the basis of `id` (see `deterministic_id`).
    /// `None` for a repo registered with no commits (id fell back to the path).
    pub root_commit_sha: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct RepositoryStore;

impl RepositoryStore {
    pub async fn list(pool: &PgPool) -> Result<Vec<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, root_commit_sha, created_at FROM repositories ORDER BY name",
        )
        .fetch_all(pool)
        .await
    }

    /// Register (or re-register) a repository under its deterministic id.
    ///
    /// `root_commit_sha` is read from disk by the caller (an engine-layer
    /// concern — `git_ops::root_commit_sha`); this DB layer stays git-free and
    /// derives the id purely. The upsert is keyed on the deterministic `id` and
    /// collapses two stale-state hazards atomically:
    ///  1. a legacy row still occupying `path` under a *different* (random) id —
    ///     deleted first so the unique `path` is free for the deterministic row;
    ///  2. the same git history already registered at a *different* path (same
    ///     deterministic id) — the id-keyed `ON CONFLICT` moves it to `path`.
    ///
    /// A transaction-scoped advisory lock on the path serializes concurrent
    /// registrations of the same path, so the collapse is atomic even when two
    /// callers derive different ids for one path (see the lock comment below).
    ///
    /// Result: exactly one row, keyed by the deterministic id, at the current path.
    pub async fn add(
        pool: &PgPool,
        name: &str,
        path: &str,
        description: Option<&str>,
        root_commit_sha: Option<&str>,
    ) -> Result<Repository, sqlx::Error> {
        let id = deterministic_id(root_commit_sha, path);
        let mut tx = pool.begin().await?;
        // Serialize concurrent registrations of the SAME path. The collapse
        // below is keyed on the deterministic `id`, but `path` carries its own
        // UNIQUE constraint that the id arbiter does not cover. Two concurrent
        // registrations that derive DIFFERENT ids for one path race between the
        // DELETE and the INSERT and collide on `path` — surfacing as a spurious
        // 409. That happens whenever the id varies for a path: e.g. one caller
        // resolves the root-commit sha while another transiently falls back to
        // the path-derived id because `root_commit_sha` returned None under
        // concurrent git load. A transaction-scoped advisory lock on the path
        // makes the whole collapse atomic per path; distinct paths never block.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(path)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM repositories WHERE path = $1 AND id <> $2")
            .bind(path)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let repo = sqlx::query_as::<_, Repository>(
            "INSERT INTO repositories (id, name, path, description, root_commit_sha) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET \
                 name = EXCLUDED.name, \
                 path = EXCLUDED.path, \
                 description = COALESCE(EXCLUDED.description, repositories.description), \
                 root_commit_sha = EXCLUDED.root_commit_sha \
             RETURNING id, name, path, description, root_commit_sha, created_at",
        )
        .bind(id)
        .bind(name)
        .bind(path)
        .bind(description)
        .bind(root_commit_sha)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(repo)
    }

    pub async fn get(pool: &PgPool, id: Uuid) -> Result<Option<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, root_commit_sha, created_at FROM repositories WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn get_by_name(pool: &PgPool, name: &str) -> Result<Option<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, root_commit_sha, created_at FROM repositories WHERE LOWER(name) = LOWER($1)",
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

    /// Idempotent upsert under the deterministic id — inserts a repository, or
    /// re-points the existing row at this path to its deterministic id (rewriting
    /// a legacy random id) and refreshes its name. Delegates to [`Self::add`] so
    /// the stale-row collapse and id derivation stay in one place; the existing
    /// description is preserved (passes `None`).
    pub async fn ensure_exists(
        pool: &PgPool,
        name: &str,
        path: &str,
        root_commit_sha: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        Self::add(pool, name, path, None, root_commit_sha).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[test]
    fn deterministic_id_same_root_commit_ignores_path() {
        // Same git history at two different paths → one identity (move/clone safe).
        let a = deterministic_id(Some("0123abc"), "/Users/me/projects/lucidos");
        let b = deterministic_id(Some("0123abc"), "/Users/me/IdeaProjects/lucidos");
        assert_eq!(a, b);
    }

    #[test]
    fn deterministic_id_differs_per_root_commit() {
        assert_ne!(
            deterministic_id(Some("aaa"), "/p"),
            deterministic_id(Some("bbb"), "/p")
        );
    }

    #[test]
    fn deterministic_id_falls_back_to_path_without_commit() {
        let a = deterministic_id(None, "/path/one");
        assert_eq!(
            a,
            deterministic_id(None, "/path/one"),
            "path fallback is stable"
        );
        assert_ne!(
            a,
            deterministic_id(None, "/path/two"),
            "different path → different id"
        );
        // An empty/whitespace SHA is treated as "no commit" → path fallback.
        assert_eq!(deterministic_id(Some("   "), "/path/one"), a);
    }

    #[tokio::test]
    async fn add_uses_deterministic_id_stable_across_readd() {
        let (pool, db) = setup_test_db().await;
        let sha = "0123456789abcdef0123456789abcdef01234567";

        let r1 = RepositoryStore::add(&pool, "Lucidos", "/tmp/repo-a", None, Some(sha))
            .await
            .unwrap();
        assert_eq!(r1.id, deterministic_id(Some(sha), "/tmp/repo-a"));
        assert_eq!(r1.root_commit_sha.as_deref(), Some(sha));

        // Remove + re-add (even renamed) yields the SAME id — never re-orphans.
        RepositoryStore::remove(&pool, r1.id).await.unwrap();
        let r2 = RepositoryStore::add(&pool, "Renamed", "/tmp/repo-a", None, Some(sha))
            .await
            .unwrap();
        assert_eq!(r1.id, r2.id);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn add_rewrites_legacy_random_id_at_same_path() {
        let (pool, db) = setup_test_db().await;
        // Legacy state: a random-id row already occupies the path.
        let legacy = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO repositories (id, name, path) VALUES ($1, 'Lucidos', '/tmp/lucidos')",
        )
        .bind(legacy)
        .execute(&pool)
        .await
        .unwrap();

        let det = deterministic_id(Some("abc"), "/tmp/lucidos");
        let r = RepositoryStore::add(&pool, "Lucidos", "/tmp/lucidos", None, Some("abc"))
            .await
            .unwrap();
        assert_eq!(r.id, det);
        assert_ne!(r.id, legacy, "legacy random id was rewritten");

        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM repositories WHERE path = '/tmp/lucidos'")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1, "exactly one row at the path");
        assert_eq!(rows[0].0, det);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn add_collapses_same_history_at_new_path() {
        let (pool, db) = setup_test_db().await;
        let sha = "deadbeefcafe";
        let a = RepositoryStore::add(&pool, "R", "/tmp/old", None, Some(sha))
            .await
            .unwrap();
        // Same history re-registered at a new path → same id, single row, path moved.
        let b = RepositoryStore::add(&pool, "R", "/tmp/new", None, Some(sha))
            .await
            .unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.path, "/tmp/new");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE id = $1")
            .bind(a.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "collapsed to one row");
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn add_is_concurrency_safe_for_same_path_with_divergent_ids() {
        // Regression: concurrent registrations of the SAME path that derive
        // DIFFERENT deterministic ids (one resolves a root-commit sha, the
        // other falls back to the path id when `root_commit_sha` transiently
        // returns None under concurrent git load) must converge to a single
        // row — not collide on the `path` UNIQUE constraint and surface a
        // spurious 409. Reproduces the e2e api `repo_files_test` flake where
        // ~12 parallel tests register the e2e workspace path at once.
        let (pool, db) = setup_test_db().await;
        let path = "/tmp/concurrent-repo";

        let mut handles = Vec::new();
        for i in 0..24 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                // Alternate between a stable sha-derived id and the path-fallback
                // id so the two id families compete for the same `path`.
                let sha = if i % 2 == 0 { Some("cafef00d") } else { None };
                RepositoryStore::add(&pool, &format!("name-{i}"), path, None, sha).await
            }));
        }
        for h in handles {
            h.await
                .expect("task panicked")
                .expect("concurrent add of the same path must not error");
        }

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM repositories WHERE path = $1")
            .bind(path)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "exactly one row at the path after concurrent adds"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn ensure_exists_preserves_existing_description() {
        let (pool, db) = setup_test_db().await;
        RepositoryStore::add(
            &pool,
            "Lucidos",
            "/tmp/x",
            Some("Canonical checkout"),
            Some("s"),
        )
        .await
        .unwrap();
        // ensure_exists passes no description, so it must not wipe the existing one.
        RepositoryStore::ensure_exists(&pool, "Lucidos", "/tmp/x", Some("s"))
            .await
            .unwrap();
        let desc: Option<String> =
            sqlx::query_scalar("SELECT description FROM repositories WHERE path = '/tmp/x'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(desc.as_deref(), Some("Canonical checkout"));
        teardown_test_db(&db).await;
    }
}
