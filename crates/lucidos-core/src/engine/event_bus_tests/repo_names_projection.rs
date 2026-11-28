use super::super::*;
use super::*;

async fn repo_name(pool: &PgPool, repo_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT name FROM repo_names WHERE id = $1")
        .bind(repo_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

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
        name: None,
        root_path: None,
        description: None,
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
