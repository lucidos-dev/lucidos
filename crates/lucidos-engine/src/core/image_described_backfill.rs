//! One-shot, idempotent backfill of `ImageDescribed` events from legacy
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

/// Stable namespace for `Uuid::new_v5` so re-runs and reboots produce the
/// same event ids and the `INSERT ... ON CONFLICT DO NOTHING` gate stays
/// effective. NAMESPACE_OID is one of the four built-in v5 namespaces; using
/// it keeps the derivation reproducible without inventing a private namespace.
const BACKFILL_NAMESPACE: Uuid = Uuid::NAMESPACE_OID;

/// Page size mirrors `image_migration::MIGRATION_BATCH_SIZE` — each row's
/// payload is small (text + small string array), so memory pressure is tiny;
/// the cap exists to avoid loading a multi-year events table at once.
const BACKFILL_BATCH_SIZE: i64 = 200;

/// Compute the deterministic event id for a backfilled `ImageDescribed`
/// row. Collapses to the same value on every run for the same
/// `(source_event_id, hash)` pair.
pub fn derive_backfill_id(source_event_id: Uuid, hash: &str) -> Uuid {
    let key = format!("{}:{}", source_event_id, hash);
    Uuid::new_v5(&BACKFILL_NAMESPACE, key.as_bytes())
}

/// Walk legacy `MessageReceived` rows with a non-null `image_description`
/// payload field and emit one `ImageDescribed` event per attached
/// `user_image_hashes` entry. Returns the number of `ImageDescribed` rows
/// inserted (across all sources). Re-runs return 0.
///
/// Skips legacy rows with no surviving hashes (e.g. multi-image messages
/// where every image was dropped during the base64 → blob migration). Those
/// would produce zero events anyway — there's nothing to attach the
/// description to.
///
/// Performance notes: the gate query relies on a sequential scan over
/// `MessageReceived` rows with a JSONB key check — there's no functional
/// index on `payload->>'image_description'`. On a workspace with thousands
/// of legacy image rows this is acceptable for a one-shot startup pass
/// (the LEFT JOIN anti-join makes resumes cheap and the gate stops matching
/// once a source has its events). Inserts run one-per-hash for simplicity;
/// hash counts per message are typically 1–3 so a multi-row batched
/// `INSERT` would only save a handful of round trips per legacy source.
pub async fn backfill_image_described_from_legacy_payload(
    pool: &PgPool,
    event_bus: &EventBus,
) -> Result<usize, sqlx::Error> {
    backfill_with_batch_size(pool, event_bus, BACKFILL_BATCH_SIZE).await
}

/// Batch size is a parameter so tests can drive multi-batch walks without
/// inserting hundreds of rows; production always passes
/// [`BACKFILL_BATCH_SIZE`] via the public wrapper above.
async fn backfill_with_batch_size(
    pool: &PgPool,
    event_bus: &EventBus,
    batch_size: i64,
) -> Result<usize, sqlx::Error> {
    let mut total_inserted = 0usize;
    // Keyset cursor over the monotonic `events.sequence` column. Skipped rows
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
        // Anti-join `LEFT JOIN ... WHERE described.id IS NULL` skips any
        // legacy row that already has an `ImageDescribed` event attached
        // (same `source_event_id`), so a partially-completed prior run is
        // resumed without re-walking already-backfilled rows.
        //
        // `thread_id` comes from the dedicated `events.thread_id` column,
        // not from the payload — `make_message_received_with_origin`
        // never sets a `thread_id` field in the payload, so reading
        // `payload->>'thread_id'` here would always be NULL and the
        // emitted `ImageDescribed` rows would have NULL `aggregate_id`.
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
                // Legacy MessageReceived rows are always inserted with a
                // non-null `thread_id` column by the EventBus persist
                // path. A NULL here means the row predates the column or
                // was inserted by something else — either way we have
                // nothing to attach the `ImageDescribed` to.
                crate::log!(
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
                // No surviving hashes (typically a row whose images were
                // dropped during the base64 → blob migration). Nothing to
                // attach an ImageDescribed to; the sequence cursor advances
                // past the row, so the permanent skip can neither loop the
                // walk nor strand later rows.
                crate::log!(
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
                // Route through `EventBus::replay_historical_event`, the
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
                        // No original wall-clock for the description: the
                        // legacy field was overwritten in place on the source
                        // MessageReceived's payload, so the description's
                        // creation time wasn't recorded. Default to NOW() —
                        // sequence ordering preserves causality regardless.
                        created: None,
                    })
                    .await?;
                if inserted.is_some() {
                    total_inserted += 1;
                }
            }
        }
    }
    if total_inserted > 0 {
        crate::log!(
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

    /// Single-image legacy row: backfill emits exactly one `ImageDescribed`
    /// event with the right `source_event_id`, `hash`, `description`, and
    /// `model = "backfill"`.
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
        // Assert payload AND column values (`aggregate_id`, `thread_id`):
        // an earlier draft read thread_id from the payload (always null on
        // real MessageReceived rows, since the constructor doesn't write
        // it there), which silently inserted ImageDescribed rows with
        // NULL aggregate_id / thread_id and broke every downstream join.
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

    /// Multi-image legacy row: backfill emits one event per attached hash,
    /// all carrying the same description text. The set of `(source, hash)`
    /// pairs is what consumers join on.
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

    /// Re-running the backfill is a no-op: the deterministic v5 event id
    /// collides on the primary key and `ON CONFLICT (id) DO NOTHING` skips
    /// the insert. This is the gate that makes startup-time invocation safe.
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

    /// A legacy row WITH a description but NO surviving hashes matches the
    /// SQL gate on every pass (it never gains an `ImageDescribed`), so the
    /// walk must advance past it instead of re-fetching it — before the
    /// sequence cursor this looped forever at startup (fixed 2026-07-07).
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

    /// A permanently-skipped row (hash-less) OLDER than a processable row must
    /// not strand the later row. Drives `batch_size = 1` so the skip fills its
    /// own batch — the shape that made the pre-cursor walk terminate on a
    /// fully-skipped batch and silently leave every later legacy row
    /// unbackfilled.
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

    /// Legacy rows without `image_description` are skipped — only rows that
    /// would have lost data under the refactor get a backfill event.
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

    /// Rows that already have an `ImageDescribed` event attached (e.g. from
    /// a previous partial run, or because the agentic loop emitted one
    /// post-refactor) must NOT receive duplicate backfill events.
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
        // Pre-existing ImageDescribed (post-refactor emit beat the backfill).
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

    /// Routing through `EventBus::replay_historical_event` means a live
    /// subscriber observes every backfilled `ImageDescribed` event — this is
    /// the point of replacing the raw `INSERT INTO events` with the bus
    /// entry point. Without this, a memory indexer / SSE bridge / projection
    /// catcher attached during a long-running engine startup would never see
    /// the backfilled rows.
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

        // Pull both broadcasts off the subscriber and verify they correspond
        // to the inserted ImageDescribed rows. The broadcast happens inside
        // the same task, so a small bounded recv is enough — no flake risk.
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

        // Re-run must be a true no-op: no row inserted AND no broadcast.
        let second = backfill_image_described_from_legacy_payload(&pool, &bus)
            .await
            .unwrap();
        assert_eq!(second, 0);
        // No further events on the bus.
        let no_more = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(
            no_more.is_err(),
            "re-run with ON CONFLICT skip must not re-broadcast"
        );

        drop(pool);
        teardown_test_db(&db_name).await;
    }

    /// `derive_backfill_id` is deterministic: same inputs produce the same
    /// uuid across processes — the basis for the idempotency gate.
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
