use super::*;
use std::borrow::Cow;

async fn migration_rows(pool: &PgPool) -> Vec<(i64, Vec<u8>)> {
    sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .expect("read applied migrations")
}

fn expected_rows() -> Vec<(i64, Vec<u8>)> {
    sqlx::migrate!()
        .iter()
        .map(|m| (m.version, m.checksum.to_vec()))
        .collect()
}

async fn drop_template(template: &str) {
    let mut admin = sqlx::PgConnection::connect(&admin_url())
        .await
        .expect("admin connect");
    sqlx::query(&format!(
        "ALTER DATABASE \"{template}\" WITH IS_TEMPLATE false"
    ))
    .execute(&mut admin)
    .await
    .expect("unmark template");
    sqlx::query(&format!("DROP DATABASE \"{template}\""))
        .execute(&mut admin)
        .await
        .expect("drop template");
}

/// A template no other test or worktree shares, so its build is this test's.
fn private_template() -> String {
    format!("lucidos_template_t{}", Uuid::new_v4().simple())
}

async fn clone_and_read(template: &str) -> Vec<(i64, Vec<u8>)> {
    let db = format!("lucidos_test_{}", Uuid::new_v4().simple());
    clone_template_db(template, &db).await;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&test_db_url(&db))
        .await
        .expect("connect clone");
    let rows = migration_rows(&pool).await;
    pool.close().await;
    teardown_test_db(&db).await;
    rows
}

#[tokio::test]
async fn a_test_database_carries_every_migration_of_its_branch() {
    let (pool, db) = setup_test_db().await;
    assert_eq!(migration_rows(&pool).await, expected_rows());
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_changed_migration_set_names_a_new_template() {
    let migrator = sqlx::migrate!();
    let current = template_db_name(&migrator);
    assert!(current.starts_with("lucidos_template_"), "{current}");
    assert!(
        !current.starts_with("lucidos_test_"),
        "the orphan sweep would drop it"
    );

    let mut fewer = sqlx::migrate!();
    let mut migrations = fewer.migrations.to_vec();
    migrations.pop();
    fewer.migrations = Cow::Owned(migrations);
    assert_ne!(template_db_name(&fewer), current);

    let mut edited = sqlx::migrate!();
    let mut all = edited.migrations.to_vec();
    let last = all.len() - 1;
    all[last].checksum = Cow::Owned(vec![0; 48]);
    edited.migrations = Cow::Owned(all);
    assert_ne!(template_db_name(&edited), current);
}

#[tokio::test]
async fn two_first_clones_at_once_both_get_the_full_schema() {
    let template = private_template();
    let (a, b) = tokio::join!(clone_and_read(&template), clone_and_read(&template));
    assert_eq!(a, expected_rows());
    assert_eq!(b, expected_rows());
    drop_template(&template).await;
}

#[tokio::test]
async fn a_half_built_template_is_rebuilt_rather_than_cloned() {
    let template = private_template();
    let mut admin = sqlx::PgConnection::connect(&admin_url())
        .await
        .expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE \"{template}\""))
        .execute(&mut admin)
        .await
        .expect("create unmigrated template");
    admin.close().await.ok();
    assert_eq!(clone_and_read(&template).await, expected_rows());
    drop_template(&template).await;
}
