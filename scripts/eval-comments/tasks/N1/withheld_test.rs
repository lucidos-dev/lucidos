use super::*;

/// Any error refuses the write. The message is the implementation's choice.
fn refused(result: Result<impl Sized, sqlx::Error>) -> bool {
    result.is_err()
}

/// The handed credential cannot write the restore policy, another preference
/// the catalog keeps from agents, or the restore boundary.
#[tokio::test]
async fn the_agent_credential_cannot_write_restore_authority() {
    let (pool, db_name) = setup_test_db().await;
    let url = provision(&pool, &test_db_url(&db_name)).await.unwrap();
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
    let policy = crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key();
    let other = crate::core::prefs::internal_specs()
        .find(|spec| spec.key != policy)
        .expect("the catalog keeps another key from agents")
        .key;
    sqlx::query("INSERT INTO preferences (key, value) VALUES ($1, 'ask'), ($2, 'x'), ('probe', '1')")
        .bind(policy)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) \
         VALUES (gen_random_uuid(), 'snap', now(), 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    for statement in [
        format!("INSERT INTO preferences (key, value, device_id) VALUES ('{policy}', 'allow', 'd')"),
        format!("UPDATE preferences SET value = 'allow' WHERE key = '{policy}'"),
        format!("DELETE FROM preferences WHERE key = '{other}'"),
        format!("UPDATE preferences SET key = '{policy}' WHERE key = 'probe'"),
        "DELETE FROM snapshot_restore_boundary".into(),
    ] {
        let result =
            sqlx::raw_sql(&format!("BEGIN READ WRITE; {statement}; COMMIT;")).execute(&mut conn).await;
        assert!(refused(result), "the agent credential ran: {statement}");
        sqlx::raw_sql("ROLLBACK").execute(&mut conn).await.unwrap();
    }

    let value: String = sqlx::query_scalar("SELECT value FROM preferences WHERE key = $1")
        .bind(policy)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(value, "ask");
    let boundaries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM snapshot_restore_boundary")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(boundaries, 1, "the boundary row survived");

    // An ordinary preference stays writable, and the engine itself is not fenced.
    sqlx::raw_sql(
        "BEGIN READ WRITE; UPDATE preferences SET value = '2' WHERE key = 'probe'; COMMIT;",
    )
    .execute(&mut conn)
    .await
    .expect("an ordinary preference stays writable");
    sqlx::query("UPDATE preferences SET value = 'allow' WHERE key = $1")
        .bind(policy)
        .execute(&pool)
        .await
        .expect("the engine role still writes the policy");
    conn.close().await.unwrap();
    drop_role(&pool, &db_name).await;
}
