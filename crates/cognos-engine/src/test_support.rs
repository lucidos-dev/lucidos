//! Shared helpers for unit tests that need a real Postgres connection.
//! Each call to `setup_test_db` creates a fresh database, runs migrations,
//! and returns the pool plus the database name to pass back to `teardown_test_db`.

#![cfg(test)]

use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cognos:cognos@localhost:5432/postgres".into())
}

pub async fn setup_test_db() -> (PgPool, String) {
    let base_url = admin_url();
    let db_name = format!(
        "cognos_test_{}",
        Uuid::new_v4().to_string().replace('-', "")
    );
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
        .execute(&admin_pool)
        .await
        .expect("create db");
    admin_pool.close().await;
    let test_url = base_url.replace("/postgres", &format!("/{}", db_name));
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&test_url)
        .await
        .expect("connect test db");
    sqlx::migrate!().run(&pool).await.expect("migrations");
    (pool, db_name)
}

pub async fn teardown_test_db(db_name: &str) {
    let base_url = admin_url();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url)
        .await
        .expect("admin connect");
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
        db_name
    ))
    .execute(&admin_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", db_name))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
}
