//! The ledger decides who emits a delivery, inside the event's own
//! transaction. So every test drives the real emit against a real database: a
//! mock of the statement would only assert what the mock was written to say.

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::MessageOrigin;
use crate::test_support::{
    refuse_event_type, setup_test_db, teardown_test_db, wait_until_blocked_on_locks,
};
use serde_json::json;

const EVENT_TYPE: &str = "DeployFinished";
/// The advisory key [`park_deliveries`] gates on.
const GATE: i64 = 4242;

/// A webhook to hang deliveries off. The ledger's foreign key needs a real
/// row, and nothing here reads any other column.
async fn seed_hook(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO webhooks (id, name, event_type, token_hash) VALUES ($1, $2, $3, 'x')")
        .bind(id)
        .bind(name)
        .bind(EVENT_TYPE)
        .execute(pool)
        .await
        .expect("seed webhook");
    id
}

async fn deliver(
    bus: &EventBus,
    hook: Uuid,
    key: &str,
    window_secs: i64,
) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
    bus.emit_webhook_delivery(
        EVENT_TYPE,
        json!({ "summary": "a deploy finished" }),
        MessageOrigin::Webhook {
            webhook_id: hook.to_string(),
            name: "github".into(),
        },
        &DeliveryKey {
            webhook_id: hook,
            key: key.into(),
            window_secs,
        },
    )
    .await
}

fn emitted(outcome: DeliveryOutcome) -> Uuid {
    match outcome {
        DeliveryOutcome::Emitted(id) => id,
        other => panic!("expected the delivery to emit, got {other:?}"),
    }
}

async fn events_emitted(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
        .bind(EVENT_TYPE)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn ledger_keys(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT delivery_key FROM webhook_deliveries ORDER BY delivery_key")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Age a row by rewriting its timestamp, so a window test needs no sleep.
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

/// Park every delivery inside its transaction, after its ledger row and before
/// its event row, until the returned connection unlocks [`GATE`].
async fn park_deliveries(pool: &PgPool) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    sqlx::query(&format!(
        "CREATE FUNCTION park_delivery() RETURNS trigger AS $$ BEGIN \
         IF NEW.event_type = '{EVENT_TYPE}' THEN \
           PERFORM pg_advisory_lock({GATE}); PERFORM pg_advisory_unlock({GATE}); \
         END IF; RETURN NEW; END $$ LANGUAGE plpgsql"
    ))
    .execute(pool)
    .await
    .expect("create the parking function");
    sqlx::query(
        "CREATE TRIGGER park_delivery BEFORE INSERT ON events \
         FOR EACH ROW EXECUTE FUNCTION park_delivery()",
    )
    .execute(pool)
    .await
    .expect("create the parking trigger");
    let mut gate = pool.acquire().await.expect("gate connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(GATE)
        .execute(&mut *gate)
        .await
        .expect("close the gate");
    gate
}

async fn open_gate(mut gate: sqlx::pool::PoolConnection<sqlx::Postgres>) {
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(GATE)
        .execute(&mut *gate)
        .await
        .expect("open the gate");
}

#[tokio::test]
async fn a_first_delivery_emits_and_a_resend_reports_its_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;

    let first = emitted(deliver(&bus, hook, "d-1", 3600).await.unwrap());
    let resend = deliver(&bus, hook, "d-1", 3600).await.unwrap();

    assert_eq!(resend, DeliveryOutcome::Duplicate(first));
    assert_eq!(events_emitted(&pool).await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The first copy's handler dies mid-emit: the client hung up, or the engine
/// restarted. Its transaction rolls back, so the resend delivers.
#[tokio::test]
async fn a_delivery_whose_handler_died_leaves_its_key_free() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;
    let gate = park_deliveries(&pool).await;

    let first = tokio::spawn({
        let bus = bus.clone();
        async move { deliver(&bus, hook, "d-1", 3600).await }
    });
    wait_until_blocked_on_locks(&pool, 1).await;
    first.abort();
    assert!(first.await.is_err(), "the handler was dropped mid-emit");
    open_gate(gate).await;

    let resend = deliver(&bus, hook, "d-1", 3600).await.unwrap();
    emitted(resend);
    assert_eq!(events_emitted(&pool).await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A second copy lands while the first is still in its transaction. It waits
/// on the uncommitted key, then answers with the first copy's event.
#[tokio::test]
async fn a_copy_arriving_mid_emit_waits_for_the_first() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;
    let gate = park_deliveries(&pool).await;

    let first = tokio::spawn({
        let bus = bus.clone();
        async move { deliver(&bus, hook, "d-1", 3600).await.unwrap() }
    });
    wait_until_blocked_on_locks(&pool, 1).await;
    let second = tokio::spawn({
        let bus = bus.clone();
        async move { deliver(&bus, hook, "d-1", 3600).await.unwrap() }
    });
    wait_until_blocked_on_locks(&pool, 2).await;
    open_gate(gate).await;

    let first = emitted(first.await.unwrap());
    assert_eq!(second.await.unwrap(), DeliveryOutcome::Duplicate(first));
    assert_eq!(events_emitted(&pool).await, 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_failed_emit_leaves_nothing_behind() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;
    refuse_event_type(&pool, EVENT_TYPE).await;

    assert!(deliver(&bus, hook, "d-1", 3600).await.is_err());
    assert!(ledger_keys(&pool).await.is_empty());

    sqlx::query("DROP TRIGGER refuse_event ON events")
        .execute(&pool)
        .await
        .unwrap();
    emitted(deliver(&bus, hook, "d-1", 3600).await.unwrap());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_key_past_its_window_emits_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;

    let first = emitted(deliver(&bus, hook, "d-1", 60).await.unwrap());
    backdate(&pool, hook, "d-1", 61).await;
    let second = emitted(deliver(&bus, hook, "d-1", 60).await.unwrap());

    assert_ne!(first, second);
    let recorded: Uuid = sqlx::query_scalar(
        "SELECT event_id FROM webhook_deliveries WHERE webhook_id = $1 AND delivery_key = 'd-1'",
    )
    .bind(hook)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recorded, second, "a resend now reports the newer event");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn one_hooks_delivery_says_nothing_about_another_hook() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let one = seed_hook(&pool, "github").await;
    let two = seed_hook(&pool, "github-mirror").await;

    emitted(deliver(&bus, one, "d-1", 3600).await.unwrap());
    // Two hooks fed by one sender are two subscriptions, and both fire.
    emitted(deliver(&bus, two, "d-1", 3600).await.unwrap());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn deleting_a_webhook_takes_its_rows_with_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;
    emitted(deliver(&bus, hook, "d-1", 3600).await.unwrap());

    sqlx::query("DELETE FROM webhooks WHERE id = $1")
        .bind(hook)
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        ledger_keys(&pool).await.is_empty(),
        "nothing outlives the hook it belonged to"
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn prune_drops_what_is_past_the_horizon_and_keeps_the_rest() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;
    emitted(deliver(&bus, hook, "old", 3600).await.unwrap());
    emitted(deliver(&bus, hook, "fresh", 3600).await.unwrap());
    backdate(&pool, hook, "old", MAX_WINDOW_SECS + 60).await;

    assert_eq!(DeliveryLedger::prune(&pool).await.unwrap(), 1);
    assert_eq!(ledger_keys(&pool).await, vec!["fresh".to_string()]);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The horizon and the configurable maximum are one constant. A horizon below
/// the window would drop a row the ledger still owes an answer for.
#[tokio::test]
async fn a_row_at_the_maximum_window_survives_a_prune() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let hook = seed_hook(&pool, "github").await;
    let first = emitted(deliver(&bus, hook, "d-1", MAX_WINDOW_SECS).await.unwrap());
    backdate(&pool, hook, "d-1", MAX_WINDOW_SECS - 60).await;

    assert_eq!(DeliveryLedger::prune(&pool).await.unwrap(), 0);
    assert_eq!(
        deliver(&bus, hook, "d-1", MAX_WINDOW_SECS).await.unwrap(),
        DeliveryOutcome::Duplicate(first)
    );
    pool.close().await;
    teardown_test_db(&db_name).await;
}
