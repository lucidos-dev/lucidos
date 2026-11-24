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
    let url = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
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
        "SELECT payload FROM diagnostic_bundles",
        "SELECT * FROM diagnostic_bundles",
    ] {
        assert!(
            denied(sqlx::query(query).fetch_all(&mut conn).await),
            "{query} was allowed"
        );
    }
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
}

const PREFERENCE_PROBE: &str =
    "BEGIN READ WRITE; INSERT INTO preferences (key, value) VALUES ('probe', '1'); COMMIT;";

#[tokio::test]
async fn the_agent_role_writes_no_table_even_inside_read_write() {
    let (pool, db_name) = setup_test_db().await;
    let url = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();

    let refused = sqlx::query("INSERT INTO preferences (key, value) VALUES ('probe', '1')")
        .execute(&mut conn)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("read-only"), "{refused}");

    let write = sqlx::raw_sql(PREFERENCE_PROBE).execute(&mut conn).await;
    assert!(denied(write), "an agent wrote a preference by SQL");
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
}

#[tokio::test]
async fn a_machine_made_workspace_writes_inside_read_write_but_never_a_secret() {
    let (pool, db_name) = setup_test_db().await;
    let url = provision(&pool, &test_db_url(&db_name), Some("agent"))
        .await
        .unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();

    sqlx::raw_sql(PREFERENCE_PROBE)
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

/// The restore policy and the restore boundary grant or fence authority, so the
/// handed credential must not write them (ADR 0612 decisions 8 and 9). Only a
/// machine-made workspace writes `preferences` at all, so the guard is what
/// refuses there.
#[tokio::test]
async fn a_machine_made_role_cannot_write_restore_authority_or_human_only_preferences() {
    let (pool, db_name) = setup_test_db().await;
    let url = provision(&pool, &test_db_url(&db_name), Some("agent"))
        .await
        .unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
    let policy = crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key();
    let other = crate::core::prefs::internal_specs()
        .find(|spec| spec.key != policy)
        .expect("the catalog keeps another key from agents")
        .key;
    sqlx::query(
        "INSERT INTO preferences (key, value) VALUES ($1, 'ask'), ($2, 'x'), ('probe', '1')",
    )
    .bind(policy)
    .bind(other)
    .execute(&pool)
    .await
    .unwrap();

    for statement in [
        format!("INSERT INTO preferences (key, value, device_id) VALUES ('{policy}', 'allow', 'd')"),
        format!("UPDATE preferences SET value = 'allow' WHERE key = '{policy}'"),
        format!("DELETE FROM preferences WHERE key = '{other}'"),
        format!("UPDATE preferences SET key = '{policy}' WHERE key = 'probe'"),
        "INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) \
         VALUES (gen_random_uuid(), 'snap', now(), 0)"
            .into(),
        "DELETE FROM snapshot_restore_boundary".into(),
    ] {
        let result =
            sqlx::raw_sql(&format!("BEGIN READ WRITE; {statement}; COMMIT;")).execute(&mut conn).await;
        assert!(denied(result), "the agent role ran: {statement}");
        sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
    }
    let value: String = sqlx::query_scalar("SELECT value FROM preferences WHERE key = $1")
        .bind(policy)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(value, "ask");

    // An ordinary preference stays writable, and the engine role is not fenced.
    sqlx::raw_sql(
        "BEGIN READ WRITE; UPDATE preferences SET value = '2' WHERE key = 'probe'; COMMIT;",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query("UPDATE preferences SET value = 'allow' WHERE key = $1")
        .bind(policy)
        .execute(&pool)
        .await
        .unwrap();
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
}

#[tokio::test]
async fn the_agent_role_reaches_no_other_database() {
    let (pool, db_name) = setup_test_db().await;
    let (other_pool, other_db) = setup_test_db().await;
    provision(&other_pool, &test_db_url(&other_db), None)
        .await
        .unwrap();
    let url = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();

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
    let first = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let second = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
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

    let agent_url = provision(&target, &url, None).await.unwrap();
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

#[tokio::test]
async fn every_table_is_readable_and_none_is_writable() {
    let (pool, db_name) = setup_test_db().await;
    crate::core::store::ensure_memory_entries_table(&pool)
        .await
        .unwrap();
    let url = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut failures = Vec::new();
    for table in tables {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position",
        )
        .bind(&table)
        .fetch_all(&pool)
        .await
        .unwrap();
        let readable: Vec<String> = columns
            .into_iter()
            .filter(|column| {
                let inbox_payload = table == "diagnostic_bundles" && column == "payload";
                let secret = crate::core::secret_store::SECRET_COLUMNS
                    .iter()
                    .chain(crate::core::secret_store::MOVED_SECRET_COLUMNS)
                    .any(|spec| spec.table == table && spec.column == column);
                !(inbox_payload || secret)
            })
            .collect();
        assert!(!readable.is_empty(), "no readable columns in {table}");
        let table_ident = ident(&table).unwrap();
        sqlx::query(&format!(
            "SELECT {} FROM {table_ident} LIMIT 0",
            idents(&readable).unwrap()
        ))
        .fetch_all(&mut conn)
        .await
        .unwrap_or_else(|error| panic!("cannot read {table}: {error}"));
        let column = ident(&readable[0]).unwrap();
        for statement in [
            format!("EXPLAIN INSERT INTO {table_ident} DEFAULT VALUES"),
            format!("EXPLAIN UPDATE {table_ident} SET {column} = {column} WHERE false"),
            format!("EXPLAIN DELETE FROM {table_ident} WHERE false"),
        ] {
            sqlx::raw_sql("BEGIN READ WRITE")
                .execute(&mut conn)
                .await
                .unwrap();
            if !denied(sqlx::query(&statement).execute(&mut conn).await) {
                failures.push(statement);
            }
            sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
        }
    }
    let nextval =
        sqlx::query("SELECT nextval(c.oid::regclass) FROM pg_class c WHERE c.relkind = 'S'")
            .fetch_all(&mut conn)
            .await;
    conn.close().await.unwrap();
    let role = lucidos_pg_url::agent_role(&db_name);
    let held = write_privileges(&pool, &role).await.unwrap();
    drop_role(&pool, &db_name).await;
    assert!(
        failures.is_empty(),
        "unexpected write grants: {failures:#?}"
    );
    assert!(held.is_empty(), "write privileges remain: {held:?}");
    assert!(
        matches!(nextval, Ok(ref rows) if rows.is_empty()) || denied(nextval),
        "the agent role advanced a sequence"
    );
}

#[tokio::test]
async fn new_tables_fail_closed_and_reprovision_removes_stale_write_grants() {
    let (pool, db_name) = setup_test_db().await;
    sqlx::query("CREATE TABLE future_authority (value text)")
        .execute(&pool)
        .await
        .unwrap();
    provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let role = ident(&lucidos_pg_url::agent_role(&db_name)).unwrap();
    // Every grant an earlier release could have left behind, and more.
    sqlx::raw_sql(&format!(
        "GRANT INSERT, UPDATE, DELETE, TRUNCATE ON ALL TABLES IN SCHEMA public TO {role}; \
         GRANT USAGE, UPDATE ON ALL SEQUENCES IN SCHEMA public TO {role};"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let url = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let held = write_privileges(&pool, &lucidos_pg_url::agent_role(&db_name))
        .await
        .unwrap();
    assert!(held.is_empty(), "stale write privileges survive: {held:?}");
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
    let mut failures = Vec::new();
    for statement in [
        "INSERT INTO future_authority VALUES ('forged')",
        "UPDATE future_authority SET value = 'forged'",
        "DELETE FROM future_authority",
    ] {
        sqlx::raw_sql("BEGIN READ WRITE")
            .execute(&mut conn)
            .await
            .unwrap();
        if !denied(sqlx::query(statement).execute(&mut conn).await) {
            failures.push(statement);
        }
        sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
    assert!(failures.is_empty(), "unexpected write grants: {failures:?}");
}

/// A grant the revoke cannot reach must stop the boot, never ride along.
#[tokio::test]
async fn a_write_privilege_that_survives_provisioning_fails_it() {
    let (pool, db_name) = setup_test_db().await;
    sqlx::raw_sql("CREATE TABLE open_to_all (value text); GRANT INSERT ON open_to_all TO PUBLIC;")
        .execute(&pool)
        .await
        .unwrap();
    let refused = provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap_err()
        .to_string();
    let machine_made = provision(&pool, &test_db_url(&db_name), Some("agent")).await;
    drop_role(&pool, &db_name).await;
    assert!(refused.contains("open_to_all"), "{refused}");
    assert!(machine_made.is_ok(), "{machine_made:?}");
}

/// Where PUBLIC holds `CREATE` on the schema, as on a database made before
/// Postgres 15, the role owns what it made. A boot must survive those
/// relations, or an agent could stop its own workspace starting.
#[tokio::test]
async fn relations_the_agent_role_created_do_not_fail_the_next_boot() {
    let (pool, db_name) = setup_test_db().await;
    provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let role = ident(&lucidos_pg_url::agent_role(&db_name)).unwrap();
    sqlx::raw_sql(&format!(
        "CREATE TABLE agent_made (id serial, value text); \
         CREATE VIEW agent_made_view AS SELECT value FROM agent_made; \
         CREATE MATERIALIZED VIEW agent_made_matview AS SELECT 1 AS one; \
         ALTER TABLE agent_made OWNER TO {role}; \
         ALTER VIEW agent_made_view OWNER TO {role}; \
         ALTER MATERIALIZED VIEW agent_made_matview OWNER TO {role}; \
         GRANT INSERT ON agent_made TO PUBLIC; \
         GRANT USAGE ON SEQUENCE agent_made_id_seq TO PUBLIC;"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let again = provision(&pool, &test_db_url(&db_name), None).await;
    // The role owns these, so the database cannot drop while it exists.
    sqlx::raw_sql(
        "DROP MATERIALIZED VIEW agent_made_matview; DROP VIEW agent_made_view; \
         DROP TABLE agent_made;",
    )
    .execute(&pool)
    .await
    .unwrap();
    drop_role(&pool, &db_name).await;
    assert!(again.is_ok(), "{again:?}");
}

/// A database made before Postgres 15 lets PUBLIC create in the schema.
/// Provisioning takes that away for every origin, since a role that creates a
/// table owns it and can grant on it.
#[tokio::test]
async fn the_agent_role_cannot_create_in_the_schema() {
    let (pool, db_name) = setup_test_db().await;
    for origin in [None, Some("agent")] {
        sqlx::query("GRANT CREATE ON SCHEMA public TO PUBLIC")
            .execute(&pool)
            .await
            .unwrap();
        let url = provision(&pool, &test_db_url(&db_name), origin)
            .await
            .unwrap();
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        for statement in [
            "CREATE TABLE agent_squat (value text)",
            "CREATE VIEW agent_squat_view AS SELECT 1 AS one",
            "CREATE SEQUENCE agent_squat_seq",
        ] {
            let created = sqlx::raw_sql(&format!("BEGIN READ WRITE; {statement}; COMMIT;"))
                .execute(&mut conn)
                .await;
            assert!(denied(created), "{origin:?} ran: {statement}");
            sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
        }
        conn.close().await.unwrap();
    }
    // The engine's own role still creates, as every migration does.
    sqlx::raw_sql("CREATE TABLE engine_made (value text); DROP TABLE engine_made;")
        .execute(&pool)
        .await
        .unwrap();
    drop_role(&pool, &db_name).await;
}

/// A `CREATE` right the revoke cannot reach stops the boot and names the fix.
#[tokio::test]
async fn a_schema_create_right_that_survives_provisioning_fails_it() {
    let (pool, db_name) = setup_test_db().await;
    provision(&pool, &test_db_url(&db_name), None)
        .await
        .unwrap();
    let group = ident(lucidos_pg_url::AGENT_ROLE_GROUP).unwrap();
    sqlx::query(&format!("GRANT CREATE ON SCHEMA public TO {group}"))
        .execute(&pool)
        .await
        .unwrap();
    let refused = provision(&pool, &test_db_url(&db_name), Some("agent"))
        .await
        .unwrap_err()
        .to_string();
    sqlx::query(&format!("REVOKE CREATE ON SCHEMA public FROM {group}"))
        .execute(&pool)
        .await
        .unwrap();
    drop_role(&pool, &db_name).await;
    assert!(
        refused.contains("REVOKE CREATE ON SCHEMA public"),
        "{refused}"
    );
}

#[test]
fn only_known_machine_origins_widen_grants() {
    for origin in ["agent", "demo", "eval", "e2e"] {
        assert!(is_machine_made(Some(origin)));
    }
    for origin in [
        None,
        Some("user"),
        Some("unknown"),
        Some("future"),
        Some("AGENT"),
        Some(""),
    ] {
        assert!(!is_machine_made(origin));
    }
}

#[tokio::test]
async fn origin_controls_writes_and_reclassification_removes_them() {
    let (pool, db_name) = setup_test_db().await;
    for origin in [
        Some("agent"),
        Some("demo"),
        Some("eval"),
        Some("e2e"),
        Some("user"),
        Some("unknown"),
        None,
        Some("future"),
    ] {
        let url = provision(&pool, &test_db_url(&db_name), origin)
            .await
            .unwrap();
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        for table in ["events", "preferences"] {
            sqlx::raw_sql("BEGIN READ WRITE")
                .execute(&mut conn)
                .await
                .unwrap();
            let result = sqlx::query(&format!("DELETE FROM {table} WHERE false"))
                .execute(&mut conn)
                .await;
            if is_machine_made(origin) {
                result.unwrap();
            } else {
                assert!(denied(result), "{table} writable for {origin:?}");
            }
            sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
        }
        assert!(
            denied(
                sqlx::query("SELECT payload FROM diagnostic_bundles")
                    .fetch_all(&mut conn)
                    .await
            ),
            "diagnostic payload readable for {origin:?}"
        );
        for table in READ_ONLY_TABLES {
            sqlx::raw_sql("BEGIN READ WRITE")
                .execute(&mut conn)
                .await
                .unwrap();
            assert!(
                denied(
                    sqlx::query(&format!(
                        "DELETE FROM {} WHERE false",
                        ident(table).unwrap()
                    ))
                    .execute(&mut conn)
                    .await
                ),
                "{table} writable for {origin:?}"
            );
            sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
        }
        conn.close().await.unwrap();
    }
    drop_role(&pool, &db_name).await;
}
