use super::super::*;
use super::*;

/// Fetch the projected name for a repo id, or None if no row exists.
async fn repo_name(pool: &PgPool, repo_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT name FROM repo_names WHERE id = $1")
        .bind(repo_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// `RepositoryAdded` populates the durable `repo_names` projection, and
/// `RepositoryRemoved` deliberately leaves it intact — so a removed repo's name
/// survives for the filter / thread-row resolution path.
#[tokio::test]
async fn repository_added_populates_repo_names_and_removed_retains_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let repo_id = Uuid::new_v4();
    bus.emit(BusEvent::System(SystemEvent::RepositoryAdded {
        repo_id: repo_id.to_string(),
        name: "Apple".into(),
        root_path: "/tmp/apple".into(),
        actor: None,
    }))
    .await
    .unwrap();
    assert_eq!(
        repo_name(&pool, repo_id).await.as_deref(),
        Some("Apple"),
        "RepositoryAdded must populate repo_names"
    );

    bus.emit(BusEvent::System(SystemEvent::RepositoryRemoved {
        repo_id: repo_id.to_string(),
        actor: None,
    }))
    .await
    .unwrap();
    assert_eq!(
        repo_name(&pool, repo_id).await.as_deref(),
        Some("Apple"),
        "RepositoryRemoved must NOT erase the projected name"
    );

    teardown_test_db(&db_name).await;
}

/// A rename (re-add at the same path keeps the same UUID) upserts the projected
/// name to the latest value.
#[tokio::test]
async fn repository_added_upserts_renamed_repo() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let repo_id = Uuid::new_v4();
    for name in ["Apple", "Apricot"] {
        bus.emit(BusEvent::System(SystemEvent::RepositoryAdded {
            repo_id: repo_id.to_string(),
            name: name.into(),
            root_path: "/tmp/apple".into(),
            actor: None,
        }))
        .await
        .unwrap();
    }

    assert_eq!(
        repo_name(&pool, repo_id).await.as_deref(),
        Some("Apricot"),
        "re-add with a new name upserts repo_names"
    );

    teardown_test_db(&db_name).await;
}
