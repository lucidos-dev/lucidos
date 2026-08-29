//! Handler for the `manage_repositories` tool: the agent-facing path to the
//! registered-repository registry (Settings → Coding Agents). Wraps
//! `RepositoryStore` and emits the same `Repository{Added,Removed}` events the
//! HTTP CRUD does (`api::repositories`), so the durable `repo_names` projection
//! stays current and every open client refreshes its repository list over SSE
//! instead of sitting stale until a page reload. Mirrors `manage_models`'
//! multi-action shape.
//!
//! Factored out of the `LucidosEngine` impl as a free function so the tests
//! below can drive every action against a real Postgres pool + EventBus without
//! booting the full engine, the same shape as `query_events_impl`.

use uuid::Uuid;

use super::{agent_tool_actor, ToolOutcome};
use crate::core::repositories::RepositoryStore;
use crate::engine::event_bus::EventBus;

pub(crate) async fn manage_repositories_impl(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    args: &serde_json::Value,
    thread_id: Uuid,
) -> ToolOutcome {
    let action = match args.get("action").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return Err("Error: 'action' is required (add, list, remove)".to_string()),
    };

    match action {
        "list" => list_repositories(pool).await,
        "add" => add_repository(pool, event_bus, args, thread_id).await,
        "remove" => remove_repository(pool, event_bus, args, thread_id).await,
        other => Err(format!(
            "Error: unknown action '{}'. Use 'add', 'list', or 'remove'.",
            other
        )),
    }
}

async fn list_repositories(pool: &sqlx::PgPool) -> ToolOutcome {
    match RepositoryStore::list(pool).await {
        Ok(repos) if repos.is_empty() => Ok("No repositories registered.".to_string()),
        Ok(repos) => {
            let mut out = format!("{} registered repositories:\n", repos.len());
            for r in &repos {
                // `~/…`, not the raw absolute path: a home dir named
                // `<username>@<employer-domain>` would otherwise reach
                // the model provider. `folder` inputs are re-expanded
                // on the way back in (`resolve_folder_input`), so the
                // abbreviated form stays usable.
                out.push_str(&format!(
                    "- **{}**: `{}`",
                    r.name,
                    crate::core::home_path::abbreviate_str(&r.path)
                ));
                if let Some(ref desc) = r.description {
                    out.push_str(&format!(" ({})", desc));
                }
                out.push('\n');
            }
            Ok(out)
        }
        Err(e) => Err(format!("Error: failed to list repositories: {}", e)),
    }
}

async fn add_repository(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    args: &serde_json::Value,
    thread_id: Uuid,
) -> ToolOutcome {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => return Err("Error: 'name' is required for 'add' action".to_string()),
    };
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => p,
        _ => return Err("Error: 'path' is required for 'add' action".to_string()),
    };

    let expanded = crate::core::home_path::expand(path);

    // Validate path exists and is a git repo
    if !std::path::Path::new(&expanded).exists() {
        return Err(format!(
            "Error: path does not exist: {}",
            crate::core::home_path::abbreviate_str(&expanded)
        ));
    }

    let git_check = tokio::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&expanded)
        .output()
        .await;
    match git_check {
        Ok(o) if !o.status.success() => {
            return Err(format!(
                "Error: not a git repository: {}",
                crate::core::home_path::abbreviate_str(&expanded)
            ));
        }
        Err(e) => return Err(format!("Error: failed to check git repo: {}", e)),
        _ => {}
    }

    let desc = args.get("description").and_then(|v| v.as_str());
    // Deterministic identity from the repo's root-commit SHA (read
    // from disk); None (no commits) → path-derived id inside `add`.
    let root_commit_sha =
        crate::engine::git_ops::root_commit_sha(std::path::Path::new(&expanded)).await;
    // `register` writes the row AND emits `RepositoryAdded`, which is what
    // feeds the `repo_names` projection and the SSE arm that reloads every
    // client's repository list. This tool used to call the raw row writer, so
    // an agent-registered repo stayed invisible until the user reloaded.
    match RepositoryStore::register(
        pool,
        event_bus,
        name,
        &expanded,
        desc,
        root_commit_sha.as_deref(),
        Some(agent_tool_actor(thread_id)),
    )
    .await
    {
        // `~/…`, for the same reason `list_repositories` abbreviates. The raw
        // path can carry a home dir named `<username>@<employer-domain>`, and
        // this result reaches the model provider and the persisted event.
        Ok(repo) => Ok(format!(
            "Repository '{}' registered at `{}`",
            repo.name,
            crate::core::home_path::abbreviate_str(&repo.path)
        )),
        Err(e) => Err(format!("Error: failed to add repository: {}", e)),
    }
}

async fn remove_repository(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    args: &serde_json::Value,
    thread_id: Uuid,
) -> ToolOutcome {
    let name = match args.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => return Err("Error: 'name' is required for 'remove' action".to_string()),
    };

    match RepositoryStore::get_by_name(pool, name).await {
        Ok(Some(repo)) => match RepositoryStore::unregister(
            pool,
            event_bus,
            repo.id,
            Some(agent_tool_actor(thread_id)),
        )
        .await
        {
            Ok(true) => Ok(format!("Repository '{}' removed", name)),
            Ok(false) => Err(format!(
                "Error: repository '{}' not found at remove time",
                name
            )),
            Err(e) => Err(format!("Error: failed to remove repository: {}", e)),
        },
        Ok(None) => Err(format!(
            "Error: no repository found with name '{}'. Use action 'list' to see registered repos.",
            name
        )),
        Err(e) => Err(format!("Error: failed to look up repository: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::{ActorMode, MessageOrigin};
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// A committed git repo on disk. `add` reads the root-commit SHA from it to
    /// derive the deterministic repo id.
    fn init_git_repo(dir: &std::path::Path) {
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "initial"]);
    }

    fn add_args(path: &std::path::Path) -> serde_json::Value {
        serde_json::json!({
            "action": "add",
            "name": "Example",
            "path": path.to_string_lossy(),
        })
    }

    /// The persisted event types for a repo id, oldest first.
    async fn repo_event_types(pool: &sqlx::PgPool, repo_id: &str) -> Vec<String> {
        sqlx::query_scalar(
            // `SystemEvent` is `#[serde(tag = "type", content = "data")]`, so
            // the persisted payload is the tagged envelope and the fields sit
            // under `data`. `sequence` is the bigserial emit order.
            "SELECT event_type FROM events \
             WHERE event_type IN ('RepositoryAdded', 'RepositoryRemoved') \
               AND payload->'data'->>'repo_id' = $1 \
             ORDER BY sequence",
        )
        .bind(repo_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn projected_name(pool: &sqlx::PgPool, repo_id: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT name FROM repo_names WHERE id = $1")
            .bind(repo_id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    /// The agent tool must go through the EventBus, not straight to the table:
    /// the `RepositoryAdded` event is what feeds the durable `repo_names`
    /// projection and the SSE arm that refreshes every open client's repository
    /// list. Before this, an agent-registered repo only appeared after a reload.
    #[tokio::test]
    async fn add_emits_repository_added_and_projects_the_name() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());

        let out = manage_repositories_impl(&pool, &bus, &add_args(tmp.path()), Uuid::new_v4())
            .await
            .expect("add succeeds");
        assert!(out.contains("Example"), "tool result names the repo: {out}");

        let repo = RepositoryStore::get_by_name(&pool, "Example")
            .await
            .unwrap()
            .expect("registry row written");
        assert_eq!(
            repo_event_types(&pool, &repo.id.to_string()).await,
            vec!["RepositoryAdded"],
        );
        assert_eq!(
            projected_name(&pool, repo.id).await.as_deref(),
            Some("Example"),
            "RepositoryAdded must reach the repo_names projection"
        );

        teardown_test_db(&db_name).await;
    }

    /// The emit carries an Agent actor deep-linking the thread whose agent ran
    /// the tool, so the route popover can't mislabel it as a direct user action.
    #[tokio::test]
    async fn add_stamps_the_agent_thread_as_actor() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());
        let thread_id = Uuid::new_v4();

        manage_repositories_impl(&pool, &bus, &add_args(tmp.path()), thread_id)
            .await
            .unwrap();

        let actor: serde_json::Value = sqlx::query_scalar(
            "SELECT payload->'data'->'actor' FROM events WHERE event_type = 'RepositoryAdded'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let origin: MessageOrigin =
            serde_json::from_value(actor).expect("actor is a MessageOrigin");
        match origin {
            MessageOrigin::ThreadLink {
                thread_id: linked,
                mode,
                ..
            } => {
                assert_eq!(linked, thread_id);
                assert_eq!(mode, ActorMode::Agent);
            }
            other => panic!("expected a ThreadLink agent actor, got {other:?}"),
        }

        teardown_test_db(&db_name).await;
    }

    /// Remove emits too, so the list refreshes live on every client. The
    /// projected name deliberately survives (see `repo_names` in db.md).
    #[tokio::test]
    async fn remove_emits_repository_removed_and_retains_the_projected_name() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap();
        init_git_repo(tmp.path());
        let thread_id = Uuid::new_v4();

        manage_repositories_impl(&pool, &bus, &add_args(tmp.path()), thread_id)
            .await
            .unwrap();
        let repo_id = RepositoryStore::get_by_name(&pool, "Example")
            .await
            .unwrap()
            .unwrap()
            .id;

        manage_repositories_impl(
            &pool,
            &bus,
            &serde_json::json!({ "action": "remove", "name": "Example" }),
            thread_id,
        )
        .await
        .expect("remove succeeds");

        assert!(RepositoryStore::get(&pool, repo_id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            repo_event_types(&pool, &repo_id.to_string()).await,
            vec!["RepositoryAdded", "RepositoryRemoved"],
        );
        assert_eq!(
            projected_name(&pool, repo_id).await.as_deref(),
            Some("Example"),
        );

        teardown_test_db(&db_name).await;
    }

    /// A rejected mutation must not emit: a refused path leaves no trace for
    /// SSE consumers to act on.
    #[tokio::test]
    async fn rejected_add_emits_nothing() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap(); // exists, but is not a git repo

        let err = manage_repositories_impl(&pool, &bus, &add_args(tmp.path()), Uuid::new_v4())
            .await
            .expect_err("a non-git path is rejected");
        assert!(err.contains("not a git repository"), "{err}");

        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'RepositoryAdded'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);

        teardown_test_db(&db_name).await;
    }

    /// `remove` of an unknown name is an error, not a silent no-op emit.
    #[tokio::test]
    async fn rejected_remove_emits_nothing() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let err = manage_repositories_impl(
            &pool,
            &bus,
            &serde_json::json!({ "action": "remove", "name": "ghost" }),
            Uuid::new_v4(),
        )
        .await
        .expect_err("unknown repo name is rejected");
        assert!(err.contains("no repository found"), "{err}");

        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'RepositoryRemoved'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);

        teardown_test_db(&db_name).await;
    }
}
