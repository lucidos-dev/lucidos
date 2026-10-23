use super::*;
use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};
use sqlx::Connection as _;

async fn drop_role(pool: &PgPool, db_name: &str) {
    pool.close().await;
    teardown_test_db(db_name).await;
    let admin = sqlx::PgPool::connect(&test_db_url("postgres"))
        .await
        .unwrap();
    sqlx::query(&format!(
        "DROP ROLE IF EXISTS {}",
        ident(&lucidos_pg_url::agent_role(db_name)).unwrap()
    ))
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
}

fn denied(result: Result<impl Sized, sqlx::Error>) -> bool {
    match result {
        Err(e) => e.to_string().contains("permission denied"),
        Ok(_) => false,
    }
}

#[tokio::test]
async fn the_agent_role_reads_everything_but_a_secret_column() {
    let (pool, db_name) = setup_test_db().await;
    crate::test_support::seed_credential(
        &pool,
        "openai",
        "https://api.openai.com",
        crate::core::AuthType::ApiKey,
        "sk-the-key",
    )
    .await;
    let url = provision(&pool, &test_db_url(&db_name)).await.unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();

    let name: String = sqlx::query_scalar("SELECT service_name FROM credentials")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(name, "openai");
    for query in [
        "SELECT auth_value FROM credentials",
        "SELECT * FROM credentials",
        "SELECT access_token FROM oauth_accounts",
        "SELECT private_key_pem FROM vapid_keys",
        "SELECT env FROM mcp_servers",
    ] {
        assert!(
            denied(sqlx::query(query).fetch_all(&mut conn).await),
            "{query} was allowed"
        );
    }
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
}

#[tokio::test]
async fn the_agent_role_writes_ordinary_tables_only_inside_read_write() {
    let (pool, db_name) = setup_test_db().await;
    let url = provision(&pool, &test_db_url(&db_name)).await.unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();

    let refused = sqlx::query("INSERT INTO preferences (key, value) VALUES ('probe', '1')")
        .execute(&mut conn)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("read-only"), "{refused}");

    sqlx::raw_sql(
        "BEGIN READ WRITE; \
         INSERT INTO preferences (key, value) VALUES ('probe', '1'); \
         COMMIT;",
    )
    .execute(&mut conn)
    .await
    .unwrap();

    let secret_write = sqlx::raw_sql(
        "BEGIN READ WRITE; \
         UPDATE credentials SET auth_value = 'x'; \
         COMMIT;",
    )
    .execute(&mut conn)
    .await;
    assert!(denied(secret_write), "an agent wrote a secret column");
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
}

#[tokio::test]
async fn the_agent_role_reaches_no_other_database() {
    let (pool, db_name) = setup_test_db().await;
    let (other_pool, other_db) = setup_test_db().await;
    provision(&other_pool, &test_db_url(&other_db))
        .await
        .unwrap();
    let url = provision(&pool, &test_db_url(&db_name)).await.unwrap();

    let mut elsewhere = lucidos_pg_url::PgUrl::parse(&url).unwrap();
    elsewhere.database = other_db.clone();
    let refused = sqlx::PgConnection::connect(&elsewhere.read_only_url())
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("permission denied"),
        "{refused}"
    );

    drop_role(&other_pool, &other_db).await;
    drop_role(&pool, &db_name).await;
}

#[test]
fn the_role_statement_carries_a_verifier_never_the_password() {
    let verifier = scram_verifier("not-the-verifier");
    assert!(verifier.starts_with("SCRAM-SHA-256$4096:"), "{verifier}");
    assert!(!verifier.contains("not-the-verifier"));
    assert_ne!(verifier, scram_verifier("not-the-verifier"), "salt reused");
}

#[tokio::test]
async fn each_boot_mints_a_new_password() {
    let (pool, db_name) = setup_test_db().await;
    let first = provision(&pool, &test_db_url(&db_name)).await.unwrap();
    let second = provision(&pool, &test_db_url(&db_name)).await.unwrap();
    assert_ne!(first, second);
    assert!(sqlx::PgConnection::connect(&first).await.is_err());
    sqlx::PgConnection::connect(&second)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    drop_role(&pool, &db_name).await;
}

#[test]
fn every_name_is_quoted_whole_into_sql() {
    assert_eq!(ident("lucidos_ws_agent").unwrap(), "\"lucidos_ws_agent\"");
    assert_eq!(
        ident("lucidos_e2e-test_agent").unwrap(),
        "\"lucidos_e2e-test_agent\""
    );
    assert_eq!(ident("a\"; DROP").unwrap(), "\"a\"\"; DROP\"");
    for bad in ["", &"x".repeat(64)] {
        assert!(ident(bad).is_err(), "{bad:?}");
    }
}

/// A workspace id may hold a hyphen, so its database name does too.
#[tokio::test]
async fn a_hyphenated_database_gets_its_agent_role() {
    let (pool, db_name) = setup_test_db().await;
    let hyphenated = format!("{db_name}-ws");
    let admin = sqlx::PgPool::connect(&test_db_url("postgres"))
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE {}", ident(&hyphenated).unwrap()))
        .execute(&admin)
        .await
        .unwrap();
    let url = test_db_url(&hyphenated);
    let target = sqlx::PgPool::connect(&url).await.unwrap();

    let agent_url = provision(&target, &url).await.unwrap();
    let mut agent = sqlx::PgConnection::connect(&agent_url).await.unwrap();
    let (database,): (String,) = sqlx::query_as("SELECT current_database()")
        .fetch_one(&mut agent)
        .await
        .unwrap();
    assert_eq!(database, hyphenated);
    agent.close().await.unwrap();

    target.close().await;
    for cleanup in [
        format!("DROP DATABASE {} WITH (FORCE)", ident(&hyphenated).unwrap()),
        format!(
            "DROP ROLE {}",
            ident(&lucidos_pg_url::agent_role(&hyphenated)).unwrap()
        ),
    ] {
        sqlx::query(&cleanup).execute(&admin).await.unwrap();
    }
    admin.close().await;
    drop_role(&pool, &db_name).await;
}
