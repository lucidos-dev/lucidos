//! INVARIANT: One-shot, idempotent backfill of `ContextCaptured` for *auxiliary model
//! calls* the engine made before it started recording them.
//!
//! Every row is a *reconstructed capture*: `reconstructed: true`, no `usage`
//! block, and an `estimated_total_tokens` rebuilt from what the events still
//! hold. No usage was ever recorded for these calls, so an estimate is the
//! only alternative to nothing.
//!
//! **The absent `usage` is the safety property.** A cost rollup filtering on
//! a present `usage` block still reports measured spend only. These estimates
//! cannot reach a number meaning real API cost.
//!
//! Each source function documents what its estimate can and cannot say.
//! **Thread titles are absent**: a `ThreadTitleGenerated` does not mean a
//! model ran, and no rule separates the ones that did. The gaps are named in
//! the plan, under Non-goals:
//! `docs/plans/2026-08-22-auxiliary-llm-calls-are-visible-to-token-accounting.md`.
//!
//! Idempotency: the id is `Uuid::new_v5(NAMESPACE_OID, source || ":" ||
//! purpose)`, so a rerun derives the same ids and collides.

use crate::engine::event_bus::{EventBus, HistoricalReplay};
use crate::engine::thread_events::ContextPurpose;
use crate::engine::thread_events::EventMeta;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

const BACKFILL_NAMESPACE: Uuid = Uuid::NAMESPACE_OID;

/// INVARIANT: Model name for a call whose model nobody wrote down. Never a guess from
/// today's preference, which would be wrong for every row predating the last
/// time it changed.
const UNKNOWN_MODEL: &str = "unknown";

struct Reconstruction {
    source_event_id: Uuid,
    thread_id: Uuid,
    created: DateTime<Utc>,
    request_chars: usize,
    model: String,
}

/// INVARIANT: The stable id for one reconstruction, so a rerun collides instead of
/// duplicating.
///
/// The key embeds the purpose's `Debug` name, which couples the id to that
/// variant's spelling. Renaming a `ContextPurpose` variant rederives every id
/// and re-inserts the whole backfill. A rename already breaks the wire name on
/// stored payloads, so treat the two as one change.
pub fn derive_backfill_id(source_event_id: Uuid, purpose: ContextPurpose) -> Uuid {
    let key = format!("{}:{:?}", source_event_id, purpose);
    Uuid::new_v5(&BACKFILL_NAMESPACE, key.as_bytes())
}

async fn history_ends_at(pool: &PgPool) -> Result<DateTime<Utc>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT LEAST((SELECT min(created) FROM events \
                        WHERE event_type = 'ContextCaptured' \
                          AND payload->>'purpose' IS NOT NULL \
                          AND payload->>'reconstructed' IS NULL), now())",
    )
    .fetch_one(pool)
    .await
}

pub async fn backfill_auxiliary_captures(
    pool: &PgPool,
    event_bus: &EventBus,
) -> Result<usize, sqlx::Error> {
    let until = history_ends_at(pool).await?;
    let mut total = 0usize;
    for (purpose, found) in [
        (
            ContextPurpose::ImageDescribe,
            image_describe_calls(pool, until).await,
        ),
        // INVARIANT: No `ConversationSummary` arm, deliberately. Every historical
        // summariser call really was stamped `memory`, so reconstructing one
        // under the newer purpose would relabel the past.
        (ContextPurpose::Memory, memory_calls(pool, until).await),
        (ContextPurpose::ImageGen, image_gen_calls(pool, until).await),
    ] {
        let found = match found {
            Ok(found) => found,
            Err(e) => {
                crate::log_legacy!(
                    "[AuxContextBackfill] could not read {:?} calls, skipping them: {}",
                    purpose,
                    e
                );
                continue;
            }
        };
        let written = write_missing(pool, event_bus, purpose, found).await?;
        total += written;
        if written > 0 {
            crate::log_legacy!(
                "[AuxContextBackfill] reconstructed {} {:?} call(s)",
                written,
                purpose
            );
        }
    }
    Ok(total)
}

async fn write_missing(
    pool: &PgPool,
    event_bus: &EventBus,
    purpose: ContextPurpose,
    found: Vec<Reconstruction>,
) -> Result<usize, sqlx::Error> {
    if found.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Uuid> = found
        .iter()
        .map(|c| derive_backfill_id(c.source_event_id, purpose))
        .collect();
    let present: std::collections::HashSet<Uuid> =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM events WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let mut written = 0usize;
    for (call, event_id) in found.iter().zip(ids) {
        if present.contains(&event_id) {
            continue;
        }
        let event = crate::engine::thread_events::aux_capture::auxiliary_capture(
            purpose,
            &call.model,
            call.request_chars,
            None,
            true,
        );
        let inserted = event_bus
            .replay_historical_event(HistoricalReplay {
                event_id,
                aggregate: "thread",
                aggregate_id: &call.thread_id.to_string(),
                event_type: "ContextCaptured",
                payload: &event.to_payload(&EventMeta::NONE),
                thread_id: Some(call.thread_id),
                created: Some(call.created),
                broadcast: false,
            })
            .await?;
        if inserted.is_some() {
            written += 1;
        }
    }
    Ok(written)
}

async fn image_describe_calls(
    pool: &PgPool,
    until: DateTime<Utc>,
) -> Result<Vec<Reconstruction>, sqlx::Error> {
    let rows: Vec<(String, Uuid, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT ON (d.payload->>'source_event_id') \
                d.payload->>'source_event_id', d.thread_id, d.created, d.payload->>'model' \
         FROM events d \
         WHERE d.event_type = 'ImageDescribed' \
           AND d.thread_id IS NOT NULL \
           AND d.payload->>'source_event_id' IS NOT NULL \
           AND d.created < $1 \
         ORDER BY d.payload->>'source_event_id', d.sequence ASC",
    )
    .bind(until)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(source, thread_id, created, model)| {
            Some(Reconstruction {
                source_event_id: Uuid::parse_str(&source).ok()?,
                thread_id,
                created,
                request_chars: crate::llm::auxiliary_prompts::IMAGE_DESCRIPTION_PROMPT
                    .chars()
                    .count(),
                model: match model.as_deref() {
                    Some("backfill") | Some("") | None => UNKNOWN_MODEL.to_string(),
                    Some(m) => m.to_string(),
                },
            })
        })
        .collect())
}

async fn memory_calls(
    pool: &PgPool,
    until: DateTime<Utc>,
) -> Result<Vec<Reconstruction>, sqlx::Error> {
    if !crate::core::store::memory_table_exists(pool).await? {
        return Ok(vec![]);
    }
    let rows: Vec<(Uuid, Uuid, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT DISTINCT ON (e.id) \
                e.id, e.thread_id, m.created_at, coalesce(length(e.payload::text), 0) \
         FROM memory_entries m \
         JOIN events e ON e.id = (m.source->>'id')::uuid \
         WHERE m.source->>'type' = 'event' AND e.thread_id IS NOT NULL \
           AND m.created_at < $1 \
         ORDER BY e.id, m.created_at ASC",
    )
    .bind(until)
    .fetch_all(pool)
    .await?;
    let prompt_chars = crate::llm::auxiliary_prompts::extraction_prompt_chars();
    Ok(rows
        .into_iter()
        .map(|(id, thread_id, created, content_chars)| Reconstruction {
            source_event_id: id,
            thread_id,
            created,
            request_chars: prompt_chars + content_chars.max(0) as usize,
            model: UNKNOWN_MODEL.to_string(),
        })
        .collect())
}

/// INVARIANT: One image per SUCCEEDED `generate_image` tool call. These carry no tokens
/// at all, which is why the row matters: without it the call leaves no trace
/// in the ledger whatsoever.
///
/// **A `ToolCalled` is an attempt, not a provider request.** Four checks in
/// `execute_generate_image` can refuse before it reaches the provider: no
/// image provider, the vision-misuse guard, an unsupported multi-image edit,
/// and an input image that will not resolve. Reconstructing those would
/// invent billed images that were never generated.
///
/// The pairing prefers an explicit `tool_called_event_id`, which only the
/// recovery sweep stamps. Otherwise it takes the first later result, and
/// stops at the next `generate_image` call: without that barrier a
/// result-less call borrows the success of the one after it.
///
/// A call with no result of its own is excluded. An engine that died mid-call
/// may well have paid for the image, but under-counting beats inventing.
async fn image_gen_calls(
    pool: &PgPool,
    until: DateTime<Utc>,
) -> Result<Vec<Reconstruction>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT t.id, t.thread_id, t.created, \
                coalesce(length(t.payload->'args'->>'prompt'), 0) \
         FROM events t \
         WHERE t.event_type = 'ToolCalled' \
           AND t.payload->>'name' = 'generate_image' \
           AND t.thread_id IS NOT NULL \
           AND t.created < $1 \
           AND coalesce(( \
                 SELECT coalesce((r.payload->>'success')::boolean, true) \
                 FROM events r \
                 WHERE r.thread_id = t.thread_id \
                   AND r.event_type = 'ToolResult' \
                   AND r.payload->>'name' = 'generate_image' \
                   AND (r.payload->>'tool_called_event_id' = t.id::text \
                        OR (r.payload->>'tool_called_event_id' IS NULL \
                            AND r.sequence > t.sequence \
                            AND r.sequence < coalesce(( \
                                  SELECT n.sequence FROM events n \
                                  WHERE n.thread_id = t.thread_id \
                                    AND n.event_type = 'ToolCalled' \
                                    AND n.payload->>'name' = 'generate_image' \
                                    AND n.sequence > t.sequence \
                                  ORDER BY n.sequence ASC LIMIT 1), \
                                  9223372036854775807))) \
                 ORDER BY (r.payload->>'tool_called_event_id' = t.id::text) DESC NULLS LAST, \
                          r.sequence ASC LIMIT 1), false)",
    )
    .bind(until)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, thread_id, created, prompt_chars)| Reconstruction {
            source_event_id: id,
            thread_id,
            created,
            request_chars: prompt_chars.max(0) as usize,
            model: UNKNOWN_MODEL.to_string(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::{json, Value};

    async fn insert_event(
        pool: &PgPool,
        id: Uuid,
        thread_id: Uuid,
        event_type: &str,
        payload: Value,
        created: DateTime<Utc>,
    ) {
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id, created) \
             VALUES ($1, 'thread', $2::text, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(thread_id)
        .bind(event_type)
        .bind(payload)
        .bind(thread_id)
        .bind(created)
        .execute(pool)
        .await
        .expect("insert source event");
    }

    async fn describe_call(
        pool: &PgPool,
        thread: Uuid,
        source: Uuid,
        model: &str,
        created: DateTime<Utc>,
    ) {
        insert_event(
            pool,
            Uuid::new_v4(),
            thread,
            "ImageDescribed",
            json!({
                "source_event_id": source.to_string(),
                "hash": "aaa",
                "description": "a red car",
                "model": model,
            }),
            created,
        )
        .await;
    }

    async fn reconstructions(pool: &PgPool, purpose: &str) -> Vec<(Value, DateTime<Utc>)> {
        sqlx::query_as(
            "SELECT payload, created FROM events \
             WHERE event_type = 'ContextCaptured' AND payload->>'purpose' = $1 \
               AND payload->>'reconstructed' = 'true' \
             ORDER BY sequence ASC",
        )
        .bind(purpose)
        .fetch_all(pool)
        .await
        .expect("read reconstructions")
    }

    fn day(hour: u32) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&format!("2026-03-04T{hour:02}:00:00Z"))
            .expect("fixed timestamp")
            .with_timezone(&Utc)
    }

    #[tokio::test]
    async fn a_reconstruction_carries_no_usage() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        describe_call(
            &pool,
            thread,
            Uuid::new_v4(),
            "gemini-3-flash-preview",
            day(10),
        )
        .await;

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 1);
        let rows = reconstructions(&pool, "image_describe").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0["producer"], "auxiliary");
        assert_eq!(rows[0].0["reconstructed"], json!(true));
        assert!(
            rows[0].0.get("usage").is_none(),
            "no usage was ever recorded, so none may be invented"
        );
        assert!(
            rows[0].0["estimated_total_tokens"].as_u64().unwrap() > 0,
            "the estimate is rebuilt from the prompt the live path sends"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_reconstruction_keeps_the_original_timestamp() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        describe_call(&pool, Uuid::new_v4(), Uuid::new_v4(), "m", day(10)).await;

        backfill_auxiliary_captures(&pool, &bus).await.unwrap();
        assert_eq!(reconstructions(&pool, "image_describe").await[0].1, day(10));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_rerun_writes_nothing() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        describe_call(&pool, thread, Uuid::new_v4(), "m", day(10)).await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolCalled",
            json!({ "name": "generate_image", "args": { "prompt": "a red car" } }),
            day(11),
        )
        .await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({ "name": "generate_image", "result": "ok", "success": true }),
            day(11),
        )
        .await;

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 2);
        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 0);
        assert_eq!(reconstructions(&pool, "image_describe").await.len(), 1);
        assert_eq!(reconstructions(&pool, "image_gen").await.len(), 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_title_is_never_reconstructed() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "MessageReceived",
            json!({ "text": "the auth handshake breaks" }),
            day(9),
        )
        .await;
        for (hour, title) in [(10, "Fix the auth bug"), (11, "Run nightly audit")] {
            insert_event(
                &pool,
                Uuid::new_v4(),
                thread,
                "ThreadTitleGenerated",
                json!({ "title": title }),
                day(hour),
            )
            .await;
        }

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 0);
        assert!(reconstructions(&pool, "title").await.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_multi_image_message_is_one_description_call() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let source = Uuid::new_v4();
        for hour in [10, 11, 12] {
            describe_call(&pool, thread, source, "gemini-3-flash-preview", day(hour)).await;
        }

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 1);
        let rows = reconstructions(&pool, "image_describe").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].0["model"], "gemini-3-flash-preview",
            "this is the one source that recorded its model"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_backfilled_description_model_reads_as_unknown() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        describe_call(&pool, Uuid::new_v4(), Uuid::new_v4(), "backfill", day(10)).await;

        backfill_auxiliary_captures(&pool, &bus).await.unwrap();
        assert_eq!(
            reconstructions(&pool, "image_describe").await[0].0["model"],
            UNKNOWN_MODEL
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_call_already_captured_live_is_not_reconstructed() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();

        describe_call(&pool, thread, Uuid::new_v4(), "m", day(8)).await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ContextCaptured",
            json!({
                "producer": "auxiliary",
                "purpose": "image_describe",
                "model": "gemini-3-flash-preview",
                "context_window": 0,
                "sections": [],
                "estimated_total_tokens": 100,
                "usage": { "input_tokens": 210, "output_tokens": 4,
                           "cache_read_tokens": 0, "cache_creation_tokens": 0 },
            }),
            day(9),
        )
        .await;
        describe_call(&pool, thread, Uuid::new_v4(), "m", day(9)).await;

        assert_eq!(
            backfill_auxiliary_captures(&pool, &bus).await.unwrap(),
            1,
            "only the pre-recording call is history"
        );
        let rows = reconstructions(&pool, "image_describe").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, day(8), "the reconstruction is the older call");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn everything_before_now_is_history_until_recording_begins() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        for hour in [8, 9, 10] {
            describe_call(&pool, Uuid::new_v4(), Uuid::new_v4(), "m", day(hour)).await;
        }

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 3);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_failed_image_call_is_not_reconstructed() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();

        let call = json!({ "name": "generate_image", "args": { "prompt": "a red car" } });
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolCalled",
            call.clone(),
            day(8),
        )
        .await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({ "name": "generate_image", "result": "refused", "success": false }),
            day(8),
        )
        .await;
        insert_event(&pool, Uuid::new_v4(), thread, "ToolCalled", call, day(9)).await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({ "name": "generate_image", "result": "ok", "success": true }),
            day(9),
        )
        .await;

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 1);
        let rows = reconstructions(&pool, "image_gen").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].1,
            day(9),
            "the failed call must not borrow the later call's success"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_image_call_with_no_result_is_not_reconstructed() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        insert_event(
            &pool,
            Uuid::new_v4(),
            Uuid::new_v4(),
            "ToolCalled",
            json!({ "name": "generate_image", "args": { "prompt": "a red car" } }),
            day(8),
        )
        .await;

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 0);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_result_less_call_does_not_borrow_the_next_call() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let call = json!({ "name": "generate_image", "args": { "prompt": "a red car" } });

        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolCalled",
            call.clone(),
            day(8),
        )
        .await;
        insert_event(&pool, Uuid::new_v4(), thread, "ToolCalled", call, day(9)).await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({ "name": "generate_image", "result": "ok", "success": true }),
            day(9),
        )
        .await;

        assert_eq!(
            backfill_auxiliary_captures(&pool, &bus).await.unwrap(),
            1,
            "only the second call reached a provider"
        );
        assert_eq!(reconstructions(&pool, "image_gen").await[0].1, day(9));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_explicitly_paired_result_is_used() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let orphan = Uuid::new_v4();
        let call = json!({ "name": "generate_image", "args": { "prompt": "a red car" } });

        insert_event(&pool, orphan, thread, "ToolCalled", call.clone(), day(8)).await;
        insert_event(&pool, Uuid::new_v4(), thread, "ToolCalled", call, day(9)).await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({ "name": "generate_image", "result": "ok", "success": true }),
            day(9),
        )
        .await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({
                "name": "generate_image",
                "result": "recovered",
                "success": true,
                "tool_called_event_id": orphan.to_string(),
            }),
            day(10),
        )
        .await;

        assert_eq!(
            backfill_auxiliary_captures(&pool, &bus).await.unwrap(),
            2,
            "the explicit pairing rescues the orphan the barrier alone would drop"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_legacy_image_result_without_success_counts_as_one() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _cb) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolCalled",
            json!({ "name": "generate_image", "args": { "prompt": "a red car" } }),
            day(8),
        )
        .await;
        insert_event(
            &pool,
            Uuid::new_v4(),
            thread,
            "ToolResult",
            json!({ "name": "generate_image", "result": "ok" }),
            day(8),
        )
        .await;

        assert_eq!(backfill_auxiliary_captures(&pool, &bus).await.unwrap(), 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn the_derived_id_separates_purposes() {
        let source = Uuid::new_v4();
        assert_ne!(
            derive_backfill_id(source, ContextPurpose::ImageDescribe),
            derive_backfill_id(source, ContextPurpose::Memory)
        );
        assert_eq!(
            derive_backfill_id(source, ContextPurpose::Memory),
            derive_backfill_id(source, ContextPurpose::Memory),
            "the same input must derive the same id, or reruns duplicate"
        );
    }
}
