use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// INVARIANT: Fixed namespace for deriving deterministic repository ids via UUIDv5.
/// Generated once and frozen: changing it would re-derive every repo's id and
/// re-orphan every `thread_summaries.cc_repo_id` binding — exactly the failure
/// this scheme exists to prevent.
const REPO_ID_NAMESPACE: Uuid = Uuid::from_u128(0xb6c1f4e2_5a3d_4f8b_9c2e_1d7a0f3e6b54);

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
    pub root_commit_sha: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct RepositoryStore;

struct UpsertOutcome {
    repo: Repository,
    changed: bool,
    collapsed: Option<Uuid>,
}

impl RepositoryStore {
    pub async fn list(pool: &PgPool) -> Result<Vec<Repository>, sqlx::Error> {
        sqlx::query_as::<_, Repository>(
            "SELECT id, name, path, description, root_commit_sha, created_at FROM repositories ORDER BY name",
        )
        .fetch_all(pool)
        .await
    }

    /// INVARIANT: Write (or rewrite) a repository row under its deterministic id, and
    /// report whether anything the outside world can observe actually changed.
    ///
    /// **Private on purpose.** This is the raw row write with no event; going
    /// through [`Self::register`] is what guarantees the paired
    /// `RepositoryAdded`. See the type-level doc.
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
    ///
    /// The returned flag is true when the row was created or when a
    /// user-visible field (name / path / description) moved. It is false for a
    /// no-op re-registration, which is the common case: the engine
    /// re-registers the Lucidos source repo on EVERY boot, and emitting there
    /// would add an events row per restart and re-fire every
    /// `on_event: RepositoryAdded` trigger on a plain restart.
    async fn upsert_row(
        pool: &PgPool,
        name: &str,
        path: &str,
        description: Option<&str>,
        root_commit_sha: Option<&str>,
    ) -> Result<UpsertOutcome, sqlx::Error> {
        let id = deterministic_id(root_commit_sha, path);
        let mut tx = pool.begin().await?;
        // INVARIANT: Serialize concurrent registrations of the SAME path. The collapse
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
        let prior: Option<(String, String, Option<String>)> =
            sqlx::query_as("SELECT name, path, description FROM repositories WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let collapsed: Option<Uuid> = sqlx::query_scalar(
            "DELETE FROM repositories WHERE path = $1 AND id <> $2 RETURNING id",
        )
        .bind(path)
        .bind(id)
        .fetch_optional(&mut *tx)
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
        // INVARIANT: Compare against the RETURNING row, not the inputs: the description
        // COALESCE keeps the stored value when the caller passes None, so the
        // returned row is the only honest picture of the post-write state.
        let changed = prior.is_none_or(|(name, path, description)| {
            (name, path, description)
                != (
                    repo.name.clone(),
                    repo.path.clone(),
                    repo.description.clone(),
                )
        });
        Ok(UpsertOutcome {
            repo,
            changed,
            collapsed,
        })
    }

    /// INVARIANT: Register (or re-register) a repository under its deterministic id, and
    /// announce it. The ONLY way to add a repository: see the type-level doc
    /// for why the emit is not the caller's choice.
    ///
    /// `RepositoryAdded` fires when the row was created or a user-visible field
    /// moved, never for a no-op re-registration (see [`Self::upsert_row`]).
    /// When the write collapses a row that held this path under a different id
    /// (a legacy random id, or the path-derived id a repo used before its first
    /// commit), that identity is gone from the registry and gets its own
    /// `RepositoryRemoved` first, so a consumer tracking the old id is not left
    /// pointing at a row that silently vanished.
    ///
    /// `actor` is who caused it: a device for the HTTP CRUD, the acting thread
    /// for the `manage_repositories` agent tool, `None` for the engine's own
    /// startup registration.
    pub async fn register(
        pool: &PgPool,
        event_bus: &EventBus,
        name: &str,
        path: &str,
        description: Option<&str>,
        root_commit_sha: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<Repository, sqlx::Error> {
        let UpsertOutcome {
            repo,
            changed,
            collapsed,
        } = Self::upsert_row(pool, name, path, description, root_commit_sha).await?;
        if let Some(old_id) = collapsed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::RepositoryRemoved {
                        repo_id: old_id.to_string(),
                        name: None,
                        root_path: None,
                        description: None,
                        actor: actor.clone(),
                    }),
                    "[Repositories] RepositoryRemoved",
                )
                .await;
        }
        if changed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::RepositoryAdded {
                        repo_id: repo.id.to_string(),
                        name: repo.name.clone(),
                        root_path: repo.path.clone(),
                        actor,
                    }),
                    "[Repositories] RepositoryAdded",
                )
                .await;
        }
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

    async fn delete_row(
        pool: &PgPool,
        id: Uuid,
    ) -> Result<Option<(String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as("DELETE FROM repositories WHERE id = $1 RETURNING name, path, description")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// INVARIANT: Unregister a repository and announce it. The ONLY way to remove one.
    ///
    /// `RepositoryRemoved` fires only when a row was actually deleted, so a
    /// repeated or racing delete announces once. The `repo_names` projection
    /// deliberately keeps the name (see `docs/database-schema-notes.md` § repo_names), so
    /// threads bound to the gone repo still resolve a label.
    pub async fn unregister(
        pool: &PgPool,
        event_bus: &EventBus,
        id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let Some((name, path, description)) = Self::delete_row(pool, id).await? else {
            return Ok(false);
        };
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::RepositoryRemoved {
                    repo_id: id.to_string(),
                    name: Some(name),
                    root_path: Some(path),
                    description,
                    actor,
                }),
                "[Repositories] RepositoryRemoved",
            )
            .await;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn deterministic_id_same_root_commit_ignores_path() {
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
        assert_eq!(deterministic_id(Some("   "), "/path/one"), a);
    }

    #[tokio::test]
    async fn register_emits_on_create_and_change_but_not_on_a_no_op() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let sha = "cafef00d";

        RepositoryStore::register(&pool, &bus, "Example", "/tmp/a", None, Some(sha), None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "RepositoryAdded").await, 1, "creation emits");

        RepositoryStore::register(&pool, &bus, "Example", "/tmp/a", None, Some(sha), None)
            .await
            .unwrap();
        assert_eq!(
            emitted(&pool, "RepositoryAdded").await,
            1,
            "an identical re-registration must NOT emit"
        );

        RepositoryStore::register(&pool, &bus, "Renamed", "/tmp/a", None, Some(sha), None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "RepositoryAdded").await, 2, "a rename emits");

        RepositoryStore::register(&pool, &bus, "Renamed", "/tmp/b", None, Some(sha), None)
            .await
            .unwrap();
        assert_eq!(
            emitted(&pool, "RepositoryAdded").await,
            3,
            "a path move emits"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn register_change_check_reads_the_written_row_not_the_arguments() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let sha = "d00d";

        RepositoryStore::register(
            &pool,
            &bus,
            "Example",
            "/tmp/x",
            Some("desc"),
            Some(sha),
            None,
        )
        .await
        .unwrap();
        assert_eq!(emitted(&pool, "RepositoryAdded").await, 1);

        let repo =
            RepositoryStore::register(&pool, &bus, "Example", "/tmp/x", None, Some(sha), None)
                .await
                .unwrap();
        assert_eq!(repo.description.as_deref(), Some("desc"));
        assert_eq!(
            emitted(&pool, "RepositoryAdded").await,
            1,
            "COALESCE kept the description, so nothing changed"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn unregister_emits_once_and_is_silent_when_nothing_was_removed() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let repo =
            RepositoryStore::register(&pool, &bus, "Example", "/tmp/a", None, Some("sha"), None)
                .await
                .unwrap();

        assert!(RepositoryStore::unregister(&pool, &bus, repo.id, None)
            .await
            .unwrap());
        assert_eq!(emitted(&pool, "RepositoryRemoved").await, 1);

        assert!(
            !RepositoryStore::unregister(&pool, &bus, repo.id, None)
                .await
                .unwrap(),
            "second delete removes nothing"
        );
        assert_eq!(
            emitted(&pool, "RepositoryRemoved").await,
            1,
            "and therefore announces nothing"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn register_uses_deterministic_id_stable_across_readd() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let sha = "0123456789abcdef0123456789abcdef01234567";

        let r1 =
            RepositoryStore::register(&pool, &bus, "Lucidos", "/tmp/repo-a", None, Some(sha), None)
                .await
                .unwrap();
        assert_eq!(r1.id, deterministic_id(Some(sha), "/tmp/repo-a"));
        assert_eq!(r1.root_commit_sha.as_deref(), Some(sha));

        RepositoryStore::unregister(&pool, &bus, r1.id, None)
            .await
            .unwrap();
        let r2 =
            RepositoryStore::register(&pool, &bus, "Renamed", "/tmp/repo-a", None, Some(sha), None)
                .await
                .unwrap();
        assert_eq!(r1.id, r2.id);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn register_rewrites_legacy_random_id_at_same_path() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let legacy = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO repositories (id, name, path) VALUES ($1, 'Lucidos', '/tmp/lucidos')",
        )
        .bind(legacy)
        .execute(&pool)
        .await
        .unwrap();

        let det = deterministic_id(Some("abc"), "/tmp/lucidos");
        let r = RepositoryStore::register(
            &pool,
            &bus,
            "Lucidos",
            "/tmp/lucidos",
            None,
            Some("abc"),
            None,
        )
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
        assert_eq!(
            emitted(&pool, "RepositoryAdded").await,
            1,
            "the id rewrite is a change consumers must see"
        );
        let removed: Vec<String> = sqlx::query_scalar(
            "SELECT payload->'data'->>'repo_id' FROM events WHERE event_type = 'RepositoryRemoved'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(removed, vec![legacy.to_string()]);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn register_announces_both_halves_when_the_id_changes_at_one_path() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let before = RepositoryStore::register(&pool, &bus, "R", "/tmp/r", None, None, None)
            .await
            .unwrap();
        let after = RepositoryStore::register(&pool, &bus, "R", "/tmp/r", None, Some("sha"), None)
            .await
            .unwrap();
        assert_ne!(before.id, after.id, "the first commit re-derives the id");

        assert_eq!(emitted(&pool, "RepositoryAdded").await, 2);
        let removed: Vec<String> = sqlx::query_scalar(
            "SELECT payload->'data'->>'repo_id' FROM events WHERE event_type = 'RepositoryRemoved'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(removed, vec![before.id.to_string()]);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn register_collapses_same_history_at_new_path() {
        let (pool, db) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let sha = "deadbeefcafe";
        let a = RepositoryStore::register(&pool, &bus, "R", "/tmp/old", None, Some(sha), None)
            .await
            .unwrap();
        let b = RepositoryStore::register(&pool, &bus, "R", "/tmp/new", None, Some(sha), None)
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
    async fn upsert_row_is_concurrency_safe_for_same_path_with_divergent_ids() {
        let (pool, db) = setup_test_db().await;
        let path = "/tmp/concurrent-repo";

        let mut handles = Vec::new();
        for i in 0..24 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let sha = if i % 2 == 0 { Some("cafef00d") } else { None };
                RepositoryStore::upsert_row(&pool, &format!("name-{i}"), path, None, sha).await
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
}
