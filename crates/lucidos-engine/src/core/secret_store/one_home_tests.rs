use super::*;
use crate::core::secret_store::{seal_json_values, McpServerEnv, Secret};
use crate::core::{EmailStore, MailPassword, McpEnvValue};
use crate::engine::event_bus::EventBus;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::collections::HashMap;

async fn seed_mailbox(pool: &PgPool, name: &str, password: &str) {
    EmailStore::upsert(
        pool,
        name,
        "user@example.com",
        "imap.example.com",
        993,
        "smtp.example.com",
        465,
        "user@example.com",
        true,
        true,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE email_accounts SET password = $2 WHERE name = $1")
        .bind(name)
        .bind(Secret::<EmailPassword>::new(password))
        .execute(pool)
        .await
        .unwrap();
}

async fn password_column_exists(pool: &PgPool) -> bool {
    column_exists(pool, &EmailPassword::SPEC).await.unwrap()
}

/// The "one home" invariant for mailboxes: a password only the old column
/// held becomes the account's credential, the column goes, and a second boot
/// changes nothing.
#[tokio::test]
async fn a_mailbox_password_moves_into_its_credential_and_the_column_goes() {
    let (pool, db_name) = setup_test_db().await;
    seed_mailbox(&pool, "mail", "app-password").await;

    move_at_boot(&pool).await.unwrap();
    move_at_boot(&pool).await.unwrap();

    assert!(!password_column_exists(&pool).await);
    let credential = CredentialStore::get_email_password(&pool, "mail")
        .await
        .unwrap()
        .expect("the password became a credential");
    assert_eq!(credential.service_name, "mail");
    assert_eq!(credential.stored_value(), "app-password");
    assert_eq!(credential.base_urls, vec!["smtp://smtp.example.com"]);
    let account = EmailStore::get(&pool, "mail").await.unwrap().unwrap();
    assert!(
        account.password == MailPassword::Saved("app-password".into()),
        "IMAP and SMTP read it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The credential was always the source of truth, so an existing one wins
/// over the copy, which goes with the column.
#[tokio::test]
async fn an_existing_credential_keeps_its_value_over_the_old_copy() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_mailbox(&pool, "mail", "stale-copy").await;
    CredentialStore::upsert(
        &pool,
        &bus,
        "mail",
        &[],
        AuthType::EmailPassword,
        "current",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    move_at_boot(&pool).await.unwrap();

    assert!(!password_column_exists(&pool).await);
    let account = EmailStore::get(&pool, "mail").await.unwrap().unwrap();
    assert!(account.password == MailPassword::Saved("current".into()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Another credential holding the account's name pushes the password to
/// `email:<name>`, which the mailbox lookup also resolves.
#[tokio::test]
async fn a_taken_name_moves_the_password_to_the_prefixed_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_mailbox(&pool, "work", "app-password").await;
    CredentialStore::upsert(
        &pool,
        &bus,
        "work",
        &["https://api.example.com".to_string()],
        AuthType::ApiKey,
        "sk-other",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    move_at_boot(&pool).await.unwrap();

    assert!(!password_column_exists(&pool).await);
    let account = EmailStore::get(&pool, "work").await.unwrap().unwrap();
    assert!(account.password == MailPassword::Saved("app-password".into()));
    let api_key = CredentialStore::get(&pool, "work").await.unwrap().unwrap();
    assert_eq!(
        api_key.stored_value(),
        "sk-other",
        "the other credential is untouched"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn seed_unmoved_server(pool: &PgPool, id: &str, env: &[(&str, &str)]) {
    let plain: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    sqlx::query(
        "INSERT INTO mcp_servers (id, name, command, args, env) VALUES ($1, $1, 'cmd', '[]', $2)",
    )
    .bind(id)
    .bind(seal_json_values(&McpServerEnv::SPEC, &plain).unwrap())
    .execute(pool)
    .await
    .unwrap();
}

/// The "one home" invariant for MCP servers: every value written before
/// pointers existed becomes a `secret` credential, and the server's launch
/// env still holds the same values.
#[tokio::test]
async fn mcp_env_values_move_into_credentials_and_still_reach_the_launch() {
    let (pool, db_name) = setup_test_db().await;
    seed_unmoved_server(&pool, "srv", &[("API_KEY", "sk-mcp"), ("REGION", "eu")]).await;

    move_at_boot(&pool).await.unwrap();
    move_at_boot(&pool).await.unwrap();

    let server = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
    assert_eq!(
        server.env.get("API_KEY"),
        Some(&McpEnvValue::Credential {
            credential: "mcp-srv-API_KEY".to_string()
        })
    );
    let stored = CredentialStore::get(&pool, "mcp-srv-API_KEY")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.auth_type, AuthType::Secret);
    assert_eq!(stored.stored_value(), "sk-mcp");
    assert!(stored.base_urls.is_empty(), "never sent anywhere over HTTP");
    let sealed: serde_json::Value =
        sqlx::query_scalar("SELECT env FROM mcp_servers WHERE id = 'srv'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sealed, serde_json::json!({}), "no value stays in env");

    let env = crate::core::mcp_servers::launch_env(&pool, &server)
        .await
        .unwrap();
    assert_eq!(env.get("API_KEY").map(String::as_str), Some("sk-mcp"));
    assert_eq!(env.get("REGION").map(String::as_str), Some("eu"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The move runs once per server. A plain value the user adds afterwards is a
/// choice, and a later boot leaves it where it is.
#[tokio::test]
async fn a_plain_value_added_after_the_move_stays_in_env() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let env = HashMap::from([("PORT".to_string(), McpEnvValue::Plain("3000".to_string()))]);
    McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
        .await
        .unwrap();

    move_at_boot(&pool).await.unwrap();

    let server = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
    assert_eq!(server.env, env);
    assert!(CredentialStore::get(&pool, "mcp-srv-PORT")
        .await
        .unwrap()
        .is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
