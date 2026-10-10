//! The ledger's whole job is deciding who owns a delivery, so every test here
//! runs against a real database. The decision lives in one SQL statement, and a
//! mock of that statement would only assert what the mock was written to say.

use super::*;
use sqlx::PgPool;

/// A webhook to hang claims off. The ledger's foreign key needs a real row, and
/// nothing here reads any other column.
async fn seed_hook(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO webhooks (id, name, event_type, token_hash) VALUES ($1, $2, $3, 'x')")
        .bind(id)
        .bind(name)
        .bind("DeployFinished")
        .execute(pool)
        .await
        .expect("seed webhook");
    id
}

/// Age a claim by rewriting its timestamp, so a window test needs no sleep.
async fn backdate(pool: &PgPool, hook: Uuid, key: &str, secs: i64) {
    sqlx::query(
        "UPDATE webhook_deliveries SET created = NOW() - make_interval(secs => $3) \
         WHERE webhook_id = $1 AND delivery_key = $2",
    )
    .bind(hook)
    .bind(key)
    .bind(secs as f64)
    .execute(pool)
    .await
    .expect("backdate");
}

/// Claim, asserting the caller won, and hand back the ownership token.
async fn win(pool: &PgPool, hook: Uuid, key: &str, window: i64) -> Uuid {
    match DeliveryLedger::claim(pool, hook, key, window)
        .await
        .unwrap()
    {
        Claim::Won { claim_id } => claim_id,
        other => panic!("expected to win the claim, got {other:?}"),
    }
}

#[tokio::test]
async fn a_first_delivery_wins_and_a_resend_reports_the_first_event() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;
    let event = Uuid::new_v4();

    let claim = win(&pool, hook, "d-1", 3600).await;
    DeliveryLedger::record_event(&pool, hook, "d-1", claim, event)
        .await
        .unwrap();

    let resend = DeliveryLedger::claim(&pool, hook, "d-1", 3600)
        .await
        .unwrap();
    assert_eq!(
        resend,
        Claim::Duplicate {
            event_id: Some(event)
        },
        "a resend answers with what the first delivery emitted"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_resend_arriving_mid_emit_says_duplicate_with_no_event_yet() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    // Claimed, but `record_event` has not run: the first copy is still emitting.
    win(&pool, hook, "d-1", 3600).await;
    assert_eq!(
        DeliveryLedger::claim(&pool, hook, "d-1", 3600)
            .await
            .unwrap(),
        Claim::Duplicate { event_id: None },
        "honest about having no id yet rather than claiming there is none"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn two_concurrent_copies_of_one_delivery_produce_exactly_one_winner() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    // The race the in-memory cache could not win. Both statements target the
    // same primary key, so Postgres serialises them on the row lock.
    let (a, b) = tokio::join!(
        DeliveryLedger::claim(&pool, hook, "d-1", 3600),
        DeliveryLedger::claim(&pool, hook, "d-1", 3600),
    );
    let won = [a.unwrap(), b.unwrap()]
        .iter()
        .filter(|c| matches!(c, Claim::Won { .. }))
        .count();
    assert_eq!(won, 1, "exactly one caller emits");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_claim_older_than_the_window_is_claimable_again() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;
    let event = Uuid::new_v4();

    let claim = win(&pool, hook, "d-1", 60).await;
    DeliveryLedger::record_event(&pool, hook, "d-1", claim, event)
        .await
        .unwrap();
    backdate(&pool, hook, "d-1", 61).await;

    // The window expired, so a sender reusing the id is a new delivery.
    win(&pool, hook, "d-1", 60).await;
    let carried: Option<Uuid> = sqlx::query_scalar(
        "SELECT event_id FROM webhook_deliveries WHERE webhook_id = $1 AND delivery_key = $2",
    )
    .bind(hook)
    .bind("d-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(carried.is_none(), "the takeover clears the stale event id");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn releasing_a_claim_lets_the_senders_retry_through() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    let claim = win(&pool, hook, "d-1", 3600).await;
    // The emit failed, so the claim was taken for nothing.
    DeliveryLedger::release(&pool, hook, "d-1", claim)
        .await
        .unwrap();

    // A retry after a failed emit is a real delivery, not a duplicate.
    win(&pool, hook, "d-1", 3600).await;

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn one_hooks_claim_says_nothing_about_another_hook() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let one = seed_hook(&pool, "github").await;
    let two = seed_hook(&pool, "github-mirror").await;

    win(&pool, one, "d-1", 3600).await;
    // Two hooks fed by one sender are two subscriptions, and both fire.
    win(&pool, two, "d-1", 3600).await;

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn deleting_a_webhook_takes_its_claims_with_it() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;
    win(&pool, hook, "d-1", 3600).await;

    sqlx::query("DELETE FROM webhooks WHERE id = $1")
        .bind(hook)
        .execute(&pool)
        .await
        .unwrap();

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "nothing outlives the hook it belonged to");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn prune_drops_what_is_past_the_horizon_and_keeps_the_rest() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    win(&pool, hook, "old", 3600).await;
    win(&pool, hook, "fresh", 3600).await;
    backdate(&pool, hook, "old", MAX_WINDOW_SECS + 60).await;

    assert_eq!(DeliveryLedger::prune(&pool).await.unwrap(), 1);
    let left: Vec<String> =
        sqlx::query_scalar("SELECT delivery_key FROM webhook_deliveries ORDER BY delivery_key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(left, vec!["fresh".to_string()]);

    crate::test_support::teardown_test_db(&db_name).await;
}

/// The horizon and the configurable maximum are deliberately one constant. Two
/// numbers could drift apart, and a horizon below the window would delete a
/// claim the ledger still owes an answer for.
#[tokio::test]
async fn a_claim_at_the_maximum_window_survives_a_prune() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    win(&pool, hook, "d-1", MAX_WINDOW_SECS).await;
    backdate(&pool, hook, "d-1", MAX_WINDOW_SECS - 60).await;

    assert_eq!(DeliveryLedger::prune(&pool).await.unwrap(), 0);
    assert!(matches!(
        DeliveryLedger::claim(&pool, hook, "d-1", MAX_WINDOW_SECS)
            .await
            .unwrap(),
        Claim::Duplicate { .. }
    ));

    crate::test_support::teardown_test_db(&db_name).await;
}

/// An emit slower than its own window loses the claim to a later delivery. The
/// stale owner must then write nothing, rather than stamping its event onto the
/// row that replaced it.
#[tokio::test]
async fn a_superseded_owner_cannot_record_its_event_on_the_new_claim() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    let slow = win(&pool, hook, "d-1", 60).await;
    // The slow emit is still running when its window runs out.
    backdate(&pool, hook, "d-1", 61).await;
    let successor = win(&pool, hook, "d-1", 60).await;
    assert_ne!(slow, successor, "a takeover mints a new claim");

    // The slow emit finally lands, far too late to own the key.
    DeliveryLedger::record_event(&pool, hook, "d-1", slow, Uuid::new_v4())
        .await
        .unwrap();
    let recorded: Option<Uuid> = sqlx::query_scalar(
        "SELECT event_id FROM webhook_deliveries WHERE webhook_id = $1 AND delivery_key = $2",
    )
    .bind(hook)
    .bind("d-1")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        recorded.is_none(),
        "the stale owner wrote nothing: the successor's claim is untouched"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

/// The same rule for the failure path. A stale owner releasing its own failed
/// emit must not delete the claim somebody else is now holding.
#[tokio::test]
async fn a_superseded_owner_cannot_release_the_new_claim() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    let slow = win(&pool, hook, "d-1", 60).await;
    backdate(&pool, hook, "d-1", 61).await;
    let successor = win(&pool, hook, "d-1", 60).await;

    DeliveryLedger::release(&pool, hook, "d-1", slow)
        .await
        .unwrap();

    let held: Option<Uuid> = sqlx::query_scalar(
        "SELECT claim_id FROM webhook_deliveries WHERE webhook_id = $1 AND delivery_key = $2",
    )
    .bind(hook)
    .bind("d-1")
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(
        held,
        Some(successor),
        "the successor still holds the key, so a third delivery is still a duplicate"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

/// A key nobody holds is winnable, never a duplicate.
///
/// This is the observable half of the released-underneath-us race: a holder
/// that fails and releases leaves the key free, and the next delivery must get
/// it. Answering `Duplicate` for a free key would 2xx a delivery that emitted
/// nothing, which loses it.
///
/// The interleaving itself, a release landing BETWEEN `claim`'s two statements,
/// is not reproducible without a seam inside the method. `CLAIM_ATTEMPTS` is
/// what covers it: the read finding no row is reachable only that way, and the
/// loop turns it into another attempt instead of a duplicate.
#[tokio::test]
async fn a_key_nobody_holds_is_won_rather_than_called_a_duplicate() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let hook = seed_hook(&pool, "github").await;

    // Stand in for the released holder: the key is free at read time, which is
    // exactly the state that race leaves behind.
    let first = win(&pool, hook, "d-1", 3600).await;
    DeliveryLedger::release(&pool, hook, "d-1", first)
        .await
        .unwrap();

    match DeliveryLedger::claim(&pool, hook, "d-1", 3600)
        .await
        .unwrap()
    {
        Claim::Won { claim_id } => assert_ne!(claim_id, first),
        other => panic!("an unheld key must be winnable, got {other:?}"),
    }

    crate::test_support::teardown_test_db(&db_name).await;
}
