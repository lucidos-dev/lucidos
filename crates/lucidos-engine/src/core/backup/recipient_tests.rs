use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

/// Every text the database holds about the recipient: the table and the
/// events. A backup key must appear in none of it.
async fn everything_stored(pool: &PgPool) -> String {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(b)::text FROM backup_recipient b \
         UNION ALL SELECT payload::text FROM events",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    rows.join("\n")
}

fn key(backup_key: &str) -> age::x25519::Identity {
    crypto::parse_backup_key(backup_key).unwrap()
}

fn recipient_of(backup_key: &str) -> String {
    crypto::parse_backup_key(backup_key)
        .unwrap()
        .to_public()
        .to_string()
}

#[tokio::test]
async fn a_minted_recipient_seals_nothing_until_its_key_comes_back() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let backup_key = start_setup(&pool).await.unwrap();
    let state = load(&pool).await.unwrap();
    assert_eq!(state.recipient, None, "a pending pair must not seal");
    assert_eq!(state.pending_recipient, Some(recipient_of(&backup_key)));

    let (other, _) = crypto::generate_key_pair();
    assert_eq!(
        confirm(&pool, &bus, &other, None).await.unwrap(),
        KeyCheck::Differs
    );
    assert_eq!(load(&pool).await.unwrap().recipient, None);

    assert_eq!(
        confirm(&pool, &bus, &key(&backup_key), None).await.unwrap(),
        KeyCheck::Matches
    );
    let state = load(&pool).await.unwrap();
    assert_eq!(state.recipient, Some(recipient_of(&backup_key)));
    assert_eq!(state.pending_recipient, None);
    assert!(state.secs_since_verified.is_some());

    let announced: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'BackupRecipientSet'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(announced, 1);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn confirming_with_nothing_pending_changes_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (identity, _) = crypto::generate_key_pair();
    assert_eq!(
        confirm(&pool, &bus, &identity, None).await.unwrap(),
        KeyCheck::NothingToCheck
    );
    assert_eq!(load(&pool).await.unwrap().recipient, None);
    teardown_test_db(&db).await;
}

/// Rotation is a second setup. The old recipient keeps sealing until the new
/// key comes back, so backups never pause for a rotation.
#[tokio::test]
async fn a_rotation_keeps_the_old_recipient_until_the_new_key_returns() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let first = start_setup(&pool).await.unwrap();
    confirm(&pool, &bus, &key(&first), None).await.unwrap();
    let second = start_setup(&pool).await.unwrap();
    assert_eq!(
        load(&pool).await.unwrap().recipient,
        Some(recipient_of(&first))
    );
    confirm(&pool, &bus, &key(&second), None).await.unwrap();
    assert_eq!(
        load(&pool).await.unwrap().recipient,
        Some(recipient_of(&second))
    );

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn verify_tells_the_held_key_from_another() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (other, _) = crypto::generate_key_pair();
    assert_eq!(
        verify(&pool, &other).await.unwrap(),
        KeyCheck::NothingToCheck
    );

    let backup_key = start_setup(&pool).await.unwrap();
    confirm(&pool, &bus, &key(&backup_key), None).await.unwrap();
    assert_eq!(
        verify(&pool, &key(&backup_key)).await.unwrap(),
        KeyCheck::Matches
    );
    assert_eq!(verify(&pool, &other).await.unwrap(), KeyCheck::Differs);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_own_recipient_needs_no_paste_back_and_drops_a_pending_pair() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    start_setup(&pool).await.unwrap();
    let (_, own) = crypto::generate_key_pair();
    use_own(&pool, &bus, &own, None).await.unwrap();
    let state = load(&pool).await.unwrap();
    assert_eq!(state.recipient, Some(own.to_string()));
    assert_eq!(state.pending_recipient, None);
    assert_eq!(
        state.secs_since_verified, None,
        "an own key was never checked, so Settings must not say it was"
    );
    assert!(state.secs_since_set.is_some());

    teardown_test_db(&db).await;
}

/// The invariant behind the whole design: setup, verify and rotate leave no
/// backup key in any table or event.
#[tokio::test]
async fn no_backup_key_is_ever_stored() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let first = start_setup(&pool).await.unwrap();
    confirm(&pool, &bus, &key(&first), None).await.unwrap();
    verify(&pool, &key(&first)).await.unwrap();
    let second = start_setup(&pool).await.unwrap();
    confirm(&pool, &bus, &key(&second), None).await.unwrap();

    let stored = everything_stored(&pool).await;
    assert!(!stored.contains("AGE-SECRET-KEY"), "{stored}");
    assert!(stored.contains(&recipient_of(&second)));

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_recipient_seals_ahead_of_any_legacy_key() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let workspace = tempfile::tempdir().unwrap();

    let (_, own) = crypto::generate_key_pair();
    use_own(&pool, &bus, &own, None).await.unwrap();
    let key = sealing_key(&pool, workspace.path()).await.unwrap();
    assert!(matches!(key, Some(SealingKey::Recipient(r)) if r.to_string() == own.to_string()));

    teardown_test_db(&db).await;
}
