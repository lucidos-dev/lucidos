//! INVARIANT: One-shot, idempotent backfill of `ImageDescribed` events from legacy
//! `MessageReceived.image_description` payload fields.
//!
//! Pre-refactor, the agentic loop wrote the Flash-generated description back
//! into the originating `MessageReceived`'s payload via an in-place
//! `jsonb_set` mutation (now removed). This backfill walks every legacy row
//! that still carries that field and emits one `ImageDescribed` event per
//! attached `user_image_hashes` entry, so consumers can read the description
//! through the typed event going forward instead of the deprecated payload
//! field.
//!
//! Idempotency: the inserted event id is `Uuid::new_v5(NAMESPACE_OID,
//! source_event_id || ":" || hash)` — re-runs collide on the primary key and
//! `ON CONFLICT (id) DO NOTHING` makes them no-ops. The `model` field is
//! stamped as the literal `"backfill"` because the original model identity
//! wasn't recorded.
//!
//! Run synchronously on engine startup before HTTP bind, mirroring
//! `core::image_migration::migrate_legacy_image_payloads` — readers must
//! never observe a mixed shape (some pre-backfill, some post) across the new
//! event-based path and the legacy field.

use crate::engine::event_bus::{EventBus, HistoricalReplay};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// INVARIANT: Stable namespace for `Uuid::new_v5` so re-runs and reboots produce the
/// same event ids and the `INSERT ... ON CONFLICT DO NOTHING` gate stays
/// effective. NAMESPACE_OID is one of the four built-in v5 namespaces; using
/// it keeps the derivation reproducible without inventing a private namespace.
const BACKFILL_NAMESPACE: Uuid = Uuid::NAMESPACE_OID;

const BACKFILL_BATCH_SIZE: i64 = 200;

pub fn derive_backfill_id(source_event_id: Uuid, hash: &str) -> Uuid {
    let key = format!("{}:{}", source_event_id, hash);
    Uuid::new_v5(&BACKFILL_NAMESPACE, key.as_bytes())
}

pub async fn backfill_image_described_from_legacy_payload(
    pool: &PgPool,
    event_bus: &EventBus,
) -> Result<usize, sqlx::Error> {
    backfill_with_batch_size(pool, event_bus, BACKFILL_BATCH_SIZE).await
}

async fn backfill_with_batch_size(
    pool: &PgPool,
    event_bus: &EventBus,
    batch_size: i64,
) -> Result<usize, sqlx::Error> {
    let mut total_inserted = 0usize;
    // INVARIANT: Keyset cursor over the monotonic `events.sequence` column. Skipped rows
    // (empty description, NULL thread_id, no surviving hashes) never gain an
    // `ImageDescribed` event, so they match the anti-join forever — a
    // LIMIT-only walk would either re-fetch them endlessly (infinite startup
    // loop) or, terminating on a fully-skipped batch, strand every legacy row
    // behind them. Advancing the cursor past whatever each batch returned
    // makes forward progress structural: the walk terminates exactly when the
    // remaining candidate set is empty, and skipped rows can't block later
    // ones.
    let mut cursor: i64 = 0;
    loop {
        let batch: Vec<(Uuid, Option<Uuid>, Value, i64)> = sqlx::query_as(
            "SELECT m.id, m.thread_id, m.payload, m.sequence \
             FROM events m \
             LEFT JOIN events d \
               ON d.event_type = 'ImageDescribed' \
              AND d.payload->>'source_event_id' = m.id::text \
             WHERE m.event_type = 'MessageReceived' \
               AND m.payload->>'image_description' IS NOT NULL \
               AND m.sequence > $2 \
               AND d.id IS NULL \
             ORDER BY m.sequence ASC \
             LIMIT $1",
        )
        .bind(batch_size)
        .bind(cursor)
        .fetch_all(pool)
        .await?;
        let Some(last) = batch.last() else {
            break;
        };
        cursor = last.3;
        for (source_id, thread_id, payload, _sequence) in batch {
            let description = match payload.get("image_description").and_then(|v| v.as_str()) {
                Some(d) if !d.is_empty() => d.to_string(),
                _ => continue,
            };
            let Some(thread_id) = thread_id else {
                crate::log_legacy!(
                    "[ImageDescribedBackfill] source={} has NULL thread_id column — skipping",
                    source_id
                );
                continue;
            };
            let hashes: Vec<String> = payload
                .get("user_image_hashes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|h| h.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if hashes.is_empty() {
                crate::log_legacy!(
                    "[ImageDescribedBackfill] source={} has image_description but no user_image_hashes — skipping",
                    source_id
                );
                continue;
            }
            for hash in hashes {
                let event_id = derive_backfill_id(source_id, &hash);
                let event_payload = json!({
                    "source_event_id": source_id.to_string(),
                    "hash": hash,
                    "description": description,
                    "model": "backfill",
                });
                // INVARIANT: Route through `EventBus::replay_historical_event`, the
                // dedicated entry point for historical backfills: it inserts
                // with `ON CONFLICT (id) DO NOTHING` (so retries / reboots
                // collide on the deterministic v5 id and skip), broadcasts on
                // the bus so SSE / memory indexer / any other live subscriber
                // observes the replay, and skips projection updates +
                // request_event_id validation that don't apply to a derived
                // fact about events that already happened.
                let inserted = event_bus
                    .replay_historical_event(HistoricalReplay {
                        event_id,
                        aggregate: "thread",
                        aggregate_id: &thread_id.to_string(),
                        event_type: "ImageDescribed",
                        payload: &event_payload,
                        thread_id: Some(thread_id),
                        created: None,
                        broadcast: true,
                    })
                    .await?;
                if inserted.is_some() {
                    total_inserted += 1;
                }
            }
        }
    }
    if total_inserted > 0 {
        crate::log_legacy!(
            "[ImageDescribedBackfill] backfilled {} ImageDescribed event(s) from legacy MessageReceived.image_description rows",
            total_inserted
        );
    }
    Ok(total_inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn insert_message_received(
        pool: &PgPool,
        source_id: Uuid,
        thread_id: Uuid,
        payload: Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id) \
             VALUES ($1, 'thread', $2::text, 'MessageReceived', $3, $4)",
        )
        .bind(source_id)
        .bind(thread_id)
        .bind(payload)
        .bind(thread_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn count_image_described(pool: &PgPool, source_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE event_type = 'ImageDescribed' \
               AND payload->>'source_event_id' = $1",
        )
        .bind(source_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn backfills_single_image_row() {
        let (pool, db_name) = setup_test_db().await;
        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "look at this",
                "user_image_hashes": ["abc123"],
                "image_description": "A red car",
            }),
        )
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let inserted = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(inserted, 1);
        let row: (Value, String, Option<Uuid>) = sqlx::query_as(
            "SELECT payload, aggregate_id, thread_id FROM events \
             WHERE event_type = 'ImageDescribed' \
               AND payload->>'source_event_id' = $1",
        )
        .bind(source.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        let (payload, aggregate_id, thread_id_col) = row;
        assert_eq!(payload["source_event_id"], source.to_string());
        assert_eq!(payload["hash"], "abc123");
        assert_eq!(payload["description"], "A red car");
        assert_eq!(payload["model"], "backfill");
        assert_eq!(
            aggregate_id,
            thread.to_string(),
            "aggregate_id must match the source thread, not be NULL"
        );
        assert_eq!(
            thread_id_col,
            Some(thread),
            "thread_id column must match the source thread"
        );
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn backfills_multi_image_row_with_one_event_per_hash() {
        let (pool, db_name) = setup_test_db().await;
        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "compare",
                "user_image_hashes": ["hash1", "hash2", "hash3"],
                "image_description": "Three diagrams",
            }),
        )
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let inserted = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(inserted, 3);
        let count = count_image_described(&pool, source).await;
        assert_eq!(count, 3, "one ImageDescribed per attached hash");
        let descs: Vec<(String,)> = sqlx::query_as(
            "SELECT payload->>'description' FROM events \
             WHERE event_type = 'ImageDescribed' \
               AND payload->>'source_event_id' = $1",
        )
        .bind(source.to_string())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            descs.iter().all(|(d,)| d == "Three diagrams"),
            "every backfilled event for the same source carries the same description"
        );
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn backfill_is_idempotent_on_repeated_runs() {
        let (pool, db_name) = setup_test_db().await;
        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "x",
                "user_image_hashes": ["hh"],
                "image_description": "An apple",
            }),
        )
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let first = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(first, 1);
        let second = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(
            second, 0,
            "re-runs must not insert duplicate ImageDescribed rows"
        );
        let count = count_image_described(&pool, source).await;
        assert_eq!(count, 1);
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn terminates_when_row_has_description_but_no_hashes() {
        let (pool, db_name) = setup_test_db().await;
        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "images were dropped in the blob migration",
                "user_image_hashes": [],
                "image_description": "A description with nothing to attach to",
            }),
        )
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let inserted = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            backfill_image_described_from_legacy_payload(&pool, &bus),
        )
        .await
        .expect("backfill must terminate, not loop on the hash-less row")
        .unwrap();
        assert_eq!(inserted, 0);
        let count = count_image_described(&pool, source).await;
        assert_eq!(count, 0);
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skipped_rows_do_not_strand_later_rows() {
        let (pool, db_name) = setup_test_db().await;
        let skipped_source = Uuid::new_v4();
        let valid_source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            skipped_source,
            thread,
            json!({
                "text": "images dropped in the blob migration",
                "user_image_hashes": [],
                "image_description": "orphaned description",
            }),
        )
        .await
        .unwrap();
        insert_message_received(
            &pool,
            valid_source,
            thread,
            json!({
                "text": "still has an image",
                "user_image_hashes": ["kept1"],
                "image_description": "A kept image",
            }),
        )
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let inserted = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            backfill_with_batch_size(&pool, &bus, 1),
        )
        .await
        .expect("walk must terminate, not loop on the skipped batch")
        .unwrap();
        assert_eq!(inserted, 1, "the row behind the skip must be backfilled");
        assert_eq!(count_image_described(&pool, skipped_source).await, 0);
        assert_eq!(count_image_described(&pool, valid_source).await, 1);
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_messagereceived_without_image_description() {
        let (pool, db_name) = setup_test_db().await;
        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "no images",
                "user_image_hashes": [],
            }),
        )
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let inserted = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(inserted, 0);
        let count = count_image_described(&pool, source).await;
        assert_eq!(count, 0);
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_sources_already_described_by_an_event() {
        let (pool, db_name) = setup_test_db().await;
        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "x",
                "user_image_hashes": ["pre-existing"],
                "image_description": "old field text",
            }),
        )
        .await
        .unwrap();
        let pre_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id) \
             VALUES ($1, 'thread', $2::text, 'ImageDescribed', $3, $4)",
        )
        .bind(pre_id)
        .bind(thread)
        .bind(json!({
            "source_event_id": source.to_string(),
            "hash": "pre-existing",
            "description": "live emit",
            "model": "claude-haiku-4-5",
        }))
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();

        let (bus, _cb) = EventBus::new(pool.clone());
        let inserted = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(
            inserted, 0,
            "must not duplicate when source already has an ImageDescribed"
        );
        let count = count_image_described(&pool, source).await;
        assert_eq!(count, 1, "only the pre-existing event remains");
        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn backfill_broadcasts_inserted_rows_to_event_bus_subscribers() {
        use crate::engine::event_bus::{BusEvent, SystemEvent};

        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let mut rx = bus.subscribe();

        let source = Uuid::new_v4();
        let thread = Uuid::new_v4();
        insert_message_received(
            &pool,
            source,
            thread,
            json!({
                "text": "hi",
                "user_image_hashes": ["h1", "h2"],
                "image_description": "Two cats",
            }),
        )
        .await
        .unwrap();

        let inserted = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(inserted, 2);

        let mut observed_hashes: std::collections::HashSet<String> = Default::default();
        for _ in 0..2 {
            let emitted = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
                .await
                .expect("subscriber must observe the replay within 200ms")
                .expect("recv ok");
            match emitted.typed {
                BusEvent::System(SystemEvent::DomainEvent {
                    event_type,
                    payload,
                    ..
                }) => {
                    assert_eq!(event_type, "ImageDescribed");
                    let h = payload["hash"].as_str().unwrap().to_string();
                    observed_hashes.insert(h);
                }
                other => panic!("expected DomainEvent broadcast, got {:?}", other),
            }
        }
        assert_eq!(
            observed_hashes,
            ["h1".to_string(), "h2".to_string()].into_iter().collect(),
            "subscriber saw both hashes"
        );

        let second = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(second, 0);
        let no_more = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(
            no_more.is_err(),
            "re-run with ON CONFLICT skip must not re-broadcast"
        );

        drop(pool);
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn derive_backfill_id_is_deterministic() {
        let source = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let id1 = derive_backfill_id(source, "abcd");
        let id2 = derive_backfill_id(source, "abcd");
        assert_eq!(id1, id2);
        let other = derive_backfill_id(source, "xyz");
        assert_ne!(id1, other, "different hashes must produce different ids");
    }
}
