use super::*;
use crate::core::secret_store::key_store::{replace_keeping, FileKeyStore};
use crate::core::secret_store::{
    CredentialAuthValue, McpServerEnv, OAuthAccessToken, OAuthRefreshToken, SecretColumn as _,
};
use crate::test_support::{setup_test_db, teardown_test_db};

/// A database seeded with plain rows the way a pre-ADR-0509 workspace holds
/// them.
async fn plain_database() -> (PgPool, String) {
    let (pool, db_name) = setup_test_db().await;
    seed_before_sealing(
        &pool,
        &[
            "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
             VALUES ('openai', '{}', 'api_key', 'sk-plain')",
            "INSERT INTO mcp_servers (id, name, command, args, env) \
             VALUES ('srv', 'Srv', 'cmd', '[]', '{\"API_KEY\": \"plain-key\"}')",
        ],
    )
    .await;
    (pool, db_name)
}

/// Run `statements` with the migration's CHECKs dropped, then put them back
/// `NOT VALID`, which is the state the migration leaves an old database in.
async fn seed_before_sealing(pool: &PgPool, statements: &[&str]) {
    let mut checks = Vec::new();
    for column in SECRET_COLUMNS {
        let definition: String = sqlx::query_scalar(
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = $1",
        )
        .bind(column.constraint())
        .fetch_one(pool)
        .await
        .unwrap();
        let definition = definition.trim_end_matches(" NOT VALID").to_string();
        sqlx::query(&format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            column.table,
            column.constraint()
        ))
        .execute(pool)
        .await
        .unwrap();
        checks.push((column.table, column.constraint(), definition));
    }
    for statement in statements {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    for (table, name, definition) in checks {
        sqlx::query(&format!(
            "ALTER TABLE {table} ADD CONSTRAINT {name} {definition} NOT VALID"
        ))
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn stored_auth_value(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT auth_value FROM credentials WHERE service_name = 'openai'")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn store() -> (tempfile::TempDir, FileKeyStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = FileKeyStore::new(dir.path().to_path_buf());
    (dir, store)
}

fn ready(boot: Boot) -> SecretKey {
    match boot {
        Boot::Ready(key) => key,
        Boot::Refused(message) => panic!("refused: {message}"),
    }
}

#[tokio::test]
async fn first_boot_mints_a_key_seals_every_plain_value_and_validates() {
    let (pool, db_name) = plain_database().await;
    let (_dir, store) = store();

    let key = ready(boot(&pool, &db_name, &store).await.unwrap());

    assert_eq!(store.get(&db_name).unwrap(), Some(key.clone()));
    let stored = stored_auth_value(&pool).await;
    assert_eq!(
        key.open(&CredentialAuthValue::SPEC, &stored).unwrap(),
        "sk-plain"
    );
    let env: serde_json::Value = sqlx::query_scalar("SELECT env FROM mcp_servers WHERE id = 'srv'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        key.open(&McpServerEnv::SPEC, env["API_KEY"].as_str().unwrap())
            .unwrap(),
        "plain-key"
    );
    for column in SECRET_COLUMNS {
        let validated: bool =
            sqlx::query_scalar("SELECT convalidated FROM pg_constraint WHERE conname = $1")
                .bind(column.constraint())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(validated, "{} still NOT VALID", column.constraint());
    }

    // A second boot is a no-op that keeps the key and the values.
    assert_eq!(ready(boot(&pool, &db_name, &store).await.unwrap()), key);
    assert_eq!(stored_auth_value(&pool).await, stored);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_row_with_two_secret_columns_seals_both_in_one_write() {
    let (pool, db_name) = plain_database().await;
    let (_dir, store) = store();
    let key = SecretKey::generate();
    store.put(&db_name, &key).unwrap();
    let sealed_access = key.seal(&OAuthAccessToken::SPEC, "access-b").unwrap();
    seed_before_sealing(
        &pool,
        &[
            "INSERT INTO oauth_accounts (provider, email, access_token, refresh_token, scopes) \
             VALUES ('google', 'a@example.com', 'access-a', 'refresh-a', '')",
            &format!(
                "INSERT INTO oauth_accounts (provider, email, access_token, refresh_token, scopes) \
                 VALUES ('google', 'b@example.com', '{sealed_access}', 'refresh-b', '')"
            ),
            "INSERT INTO oauth_accounts (provider, email, access_token, refresh_token, scopes) \
             VALUES ('google', 'c@example.com', 'access-c', NULL, '')",
        ],
    )
    .await;

    assert_eq!(ready(boot(&pool, &db_name, &store).await.unwrap()), key);

    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT email, access_token, refresh_token FROM oauth_accounts ORDER BY email",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let opened: Vec<(String, String, Option<String>)> = rows
        .into_iter()
        .map(|(email, access, refresh)| {
            let access = key.open(&OAuthAccessToken::SPEC, &access).unwrap();
            let refresh = refresh.map(|r| key.open(&OAuthRefreshToken::SPEC, &r).unwrap());
            (email, access, refresh)
        })
        .collect();
    let expected = [
        ("a@example.com", "access-a", Some("refresh-a")),
        ("b@example.com", "access-b", Some("refresh-b")),
        ("c@example.com", "access-c", None),
    ]
    .map(|(e, a, r)| (e.to_string(), a.to_string(), r.map(str::to_string)));
    assert_eq!(opened, expected);
    for column in [OAuthAccessToken::SPEC, OAuthRefreshToken::SPEC] {
        let validated: bool =
            sqlx::query_scalar("SELECT convalidated FROM pg_constraint WHERE conname = $1")
                .bind(column.constraint())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(validated, "{} still NOT VALID", column.constraint());
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_sealed_database_without_its_key_refuses_and_keeps_every_secret() {
    let (pool, db_name) = plain_database().await;
    let (_dir, first) = store();
    let key = ready(boot(&pool, &db_name, &first).await.unwrap());
    let sealed = stored_auth_value(&pool).await;

    let (_dir2, empty) = store();
    let Boot::Refused(message) = boot(&pool, &db_name, &empty).await.unwrap() else {
        panic!("booted without the key");
    };
    assert!(message.contains(&key.id()), "{message}");
    assert!(
        message.contains("import-key") && message.contains("discard"),
        "{message}"
    );
    assert_eq!(
        empty.get(&db_name).unwrap(),
        None,
        "minted over a sealed database"
    );
    assert_eq!(stored_auth_value(&pool).await, sealed);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_wrong_key_refuses_and_names_both() {
    let (pool, db_name) = plain_database().await;
    let (_dir, first) = store();
    let right = ready(boot(&pool, &db_name, &first).await.unwrap());

    let (_dir2, other) = store();
    let wrong = SecretKey::generate();
    other.put(&db_name, &wrong).unwrap();
    let Boot::Refused(message) = boot(&pool, &db_name, &other).await.unwrap() else {
        panic!("booted with the wrong key");
    };
    assert!(
        message.contains(&right.id()) && message.contains(&wrong.id()),
        "{message}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_key_another_workspace_parked_still_boots_its_database() {
    let (pool, db_name) = plain_database().await;
    let (_dir, store) = store();
    let ours = ready(boot(&pool, &db_name, &store).await.unwrap());

    let theirs = SecretKey::generate();
    replace_keeping(&store, &db_name, &theirs).unwrap();

    assert_eq!(ready(boot(&pool, &db_name, &store).await.unwrap()), ours);
    assert_eq!(store.get(&db_name).unwrap(), Some(theirs), "moved theirs");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_plain_write_after_the_migration_is_refused_by_the_check() {
    let (pool, db_name) = setup_test_db().await;
    let refused = sqlx::query(
        "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
         VALUES ('x', '{}', 'api_key', 'sk-plain')",
    )
    .execute(&pool)
    .await;
    assert!(refused.is_err(), "a plain secret was stored");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn import_takes_only_the_key_the_database_names() {
    let (pool, db_name) = plain_database().await;
    let (_dir, first) = store();
    let right = ready(boot(&pool, &db_name, &first).await.unwrap());

    let (_dir2, fresh) = store();
    let err = import_key(&pool, &db_name, &fresh, &SecretKey::generate())
        .await
        .unwrap_err();
    assert!(err.to_string().contains(&right.id()), "{err}");
    assert_eq!(fresh.get(&db_name).unwrap(), None);

    import_key(&pool, &db_name, &fresh, &right).await.unwrap();
    assert_eq!(ready(boot(&pool, &db_name, &fresh).await.unwrap()), right);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn discard_removes_only_what_the_key_cannot_open_then_boot_succeeds() {
    let (pool, db_name) = plain_database().await;
    let (_dir, lost) = store();
    ready(boot(&pool, &db_name, &lost).await.unwrap());

    // One value the surviving key can open, beside the lost key's values.
    let (_dir2, fresh) = store();
    let kept = SecretKey::generate();
    fresh.put(&db_name, &kept).unwrap();
    sqlx::query(
        "INSERT INTO credentials (service_name, base_urls, auth_type, auth_value) \
         VALUES ('kept', '{}', 'api_key', $1)",
    )
    .bind(kept.seal(&CredentialAuthValue::SPEC, "still-here").unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let report = discard(&pool, &db_name, &fresh).await.unwrap();
    assert!(
        report
            .iter()
            .any(|line| line.starts_with("credentials.auth_value")),
        "{report:?}"
    );
    assert!(
        report
            .iter()
            .any(|line| line.starts_with("mcp_servers.env")),
        "{report:?}"
    );

    let names: Vec<String> = sqlx::query_scalar("SELECT service_name FROM credentials")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, ["kept"]);
    let env: serde_json::Value = sqlx::query_scalar("SELECT env FROM mcp_servers WHERE id = 'srv'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        env,
        serde_json::json!({}),
        "the server stays, its lost secret goes"
    );
    assert_eq!(ready(boot(&pool, &db_name, &fresh).await.unwrap()), kept);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
