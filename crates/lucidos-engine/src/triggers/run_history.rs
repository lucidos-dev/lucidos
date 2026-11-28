use chrono::{DateTime, Utc};
use sqlx::PgPool;

const RECENT_RUN_WINDOW: i64 = 50;

/// INVARIANT: What the event store knows about one trigger's lifetime.
///
/// **Clock discipline — the two fields use different clocks on purpose.**
///
/// `last_run` is an *engine-clock* instant: what the engine itself recorded for
/// the run (see [`recorded_run_time`]). It is deliberately NOT `events.created`,
/// which is the Postgres server clock. The scheduler derives its slots from the
/// engine clock, so comparing a slot against a DB-clock timestamp is a
/// correctness bug — it is what caused the 2026-07-29 double-fire (see
/// `docs/plans/2026-07-29-cron-slot-catch-up-double-fire.md`).
///
/// `created_at` is the DB clock, because `TriggerCreated` carries no engine-clock
/// timestamp in its payload. That makes the "slot predates the trigger" check
/// **best-effort, not exact**: under the same skew, a trigger created a few
/// minutes *after* a slot can read as having been created before it, and the
/// check fails open. The failure mode is bounded — the catch-up then fires a
/// brand-new trigger once for a slot it never existed for, which is exactly the
/// behavior that existed before that check was added. It can never produce a
/// double-fire, because that is guarded by `last_run`, which is engine-clock.
/// Making it exact means persisting an engine-clock creation timestamp in the
/// `TriggerCreated` payload; deliberately out of scope here (it would only help
/// triggers created after the change, since legacy rows have no such field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerRunHistory {
    pub last_run: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
}

impl TriggerRunHistory {
    pub fn effective_last_run(
        &self,
        in_memory_last_run: Option<DateTime<Utc>>,
    ) -> Option<DateTime<Utc>> {
        self.last_run.max(in_memory_last_run)
    }
}

/// INVARIANT: The *recorded run time* of a run-marker row — the instant the engine says the
/// trigger ran, on the engine's own clock.
///
/// `record_trigger_executed` stamps `chrono::Utc::now()` into the payload as
/// `last_run`; `events.created` is a *different* clock (Postgres server time,
/// `DEFAULT NOW()`). The scheduler compares run times against slots it derived
/// from the engine clock, so mixing the two is a correctness bug, not a rounding
/// nit: on 2026-07-29 a macOS sleep left the Docker-hosted Postgres clock 280 s
/// behind, `created` landed *before* the slot the run had just served, and the
/// startup catch-up re-fired the slot (a duplicate push notification). The sleep
/// that makes a slot "missed" is the same event that skews the DB clock, so the
/// two-clock comparison fails exactly when it is load-bearing.
///
/// Falls back to `created` only for rows with no parseable `last_run` — legacy
/// events from before the field existed.
///
/// **This is the one definition**, shared by the event-store read here and the
/// replay path in [`super::replay`]. It deliberately lives in Rust rather than
/// in the SQL: expressing it as a `CASE`-guarded `::timestamptz` cast needs a
/// regex that fully validates RFC 3339, and a value that is date-shaped but
/// invalid (`2026-99-99Tbad`) slips through and aborts the whole query —
/// which would leave the catch-up permanently reporting "history unavailable".
pub(crate) fn recorded_run_time(
    payload_last_run: Option<&str>,
    created: DateTime<Utc>,
) -> DateTime<Utc> {
    payload_last_run
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(created)
}

/// INVARIANT: Read one trigger's run history from the event store.
///
/// Two indexed reads of the trigger's own aggregate stream, both covered by
/// `idx_events_aggregate_aggregate_id_seq (aggregate, aggregate_id, sequence)`.
/// Runs only when the catch-up has actually found a missed slot, so at most once
/// per trigger per task-runner registration and usually not at all.
///
/// Errors propagate: the caller is the catch-up, which must fail closed rather
/// than treat "I couldn't check" as "it hasn't run".
pub async fn load_trigger_run_history(
    pool: &PgPool,
    trigger_id: &str,
) -> Result<TriggerRunHistory, sqlx::Error> {
    let created_at: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT MAX(created) FROM events \
         WHERE aggregate = 'trigger' AND aggregate_id = $1 \
           AND event_type IN ('TriggerCreated', 'ScheduledTriggerCreated')",
    )
    .bind(trigger_id)
    .fetch_one(pool)
    .await?;

    // INVARIANT: Timestamps come back raw and are resolved in Rust by `recorded_run_time` —
    // a malformed payload value degrades that one row to its `created` instead of
    // erroring the query.
    let rows: Vec<(Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT payload->>'last_run', created FROM events \
         WHERE aggregate = 'trigger' AND aggregate_id = $1 \
           AND event_type = 'TriggerExecuted' \
         ORDER BY sequence DESC LIMIT $2",
    )
    .bind(trigger_id)
    .bind(RECENT_RUN_WINDOW)
    .fetch_all(pool)
    .await?;

    let last_run = rows
        .iter()
        .map(|(payload_last_run, created)| recorded_run_time(payload_last_run.as_deref(), *created))
        .max();

    Ok(TriggerRunHistory {
        last_run,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_trigger_event, setup_test_db, teardown_test_db};

    #[test]
    fn recorded_run_time_prefers_the_payload_and_falls_back_to_created() {
        let created = DateTime::parse_from_rfc3339("2026-07-29T05:44:35Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            recorded_run_time(Some("2026-07-29T05:49:15Z"), created).to_rfc3339(),
            "2026-07-29T05:49:15+00:00"
        );
        assert_eq!(recorded_run_time(None, created), created);
        assert_eq!(recorded_run_time(Some("not-a-timestamp"), created), created);
        assert_eq!(recorded_run_time(Some("2026-99-99Tbad"), created), created);
        assert_eq!(
            recorded_run_time(Some("2026-02-30T05:49:15Z"), created),
            created
        );
    }

    #[tokio::test]
    async fn history_prefers_engine_clock_last_run_and_takes_the_latest() {
        let (pool, db) = setup_test_db().await;
        let t = "11111111-1111-1111-1111-111111111111";

        seed_trigger_event(
            &pool,
            t,
            "TriggerCreated",
            "2026-07-01T09:00:00Z",
            serde_json::json!({ "trigger_id": t, "name": "Morning reminder" }),
        )
        .await;
        seed_trigger_event(
            &pool,
            t,
            "TriggerExecuted",
            "2026-07-28T05:45:08Z",
            serde_json::json!({ "trigger_id": t, "last_run": "2026-07-28T05:45:08Z" }),
        )
        .await;
        seed_trigger_event(
            &pool,
            t,
            "TriggerExecuted",
            "2026-07-29T05:44:35Z",
            serde_json::json!({ "trigger_id": t, "last_run": "2026-07-29T05:49:15Z" }),
        )
        .await;

        let history = load_trigger_run_history(&pool, t).await.unwrap();
        assert_eq!(
            history.last_run.unwrap().to_rfc3339(),
            "2026-07-29T05:49:15+00:00",
            "must report the engine-clock run time, not events.created"
        );
        assert_eq!(
            history.created_at.unwrap().to_rfc3339(),
            "2026-07-01T09:00:00+00:00"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn history_survives_a_date_shaped_but_invalid_payload_timestamp() {
        let (pool, db) = setup_test_db().await;
        let t = "22222222-2222-2222-2222-222222222222";

        seed_trigger_event(
            &pool,
            t,
            "TriggerExecuted",
            "2026-07-28T05:45:08Z",
            serde_json::json!({ "trigger_id": t }),
        )
        .await;
        seed_trigger_event(
            &pool,
            t,
            "TriggerExecuted",
            "2026-07-29T05:44:35Z",
            serde_json::json!({ "trigger_id": t, "last_run": "2026-99-99Tbad" }),
        )
        .await;

        let history = load_trigger_run_history(&pool, t)
            .await
            .expect("a malformed payload timestamp must not error the query");
        assert_eq!(
            history.last_run.unwrap().to_rfc3339(),
            "2026-07-29T05:44:35+00:00",
            "the bad row degrades to its own created, the others still count"
        );
        assert!(
            history.created_at.is_none(),
            "no TriggerCreated row was inserted"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn history_is_empty_for_a_trigger_that_never_ran() {
        let (pool, db) = setup_test_db().await;
        let t = "33333333-3333-3333-3333-333333333333";
        let other = "44444444-4444-4444-4444-444444444444";

        seed_trigger_event(
            &pool,
            t,
            "TriggerCreated",
            "2026-07-29T06:00:00Z",
            serde_json::json!({ "trigger_id": t, "name": "Brand new" }),
        )
        .await;
        seed_trigger_event(
            &pool,
            other,
            "TriggerExecuted",
            "2026-07-29T05:49:15Z",
            serde_json::json!({ "trigger_id": other, "last_run": "2026-07-29T05:49:15Z" }),
        )
        .await;

        let history = load_trigger_run_history(&pool, t).await.unwrap();
        assert_eq!(history.last_run, None);
        assert_eq!(
            history.created_at.unwrap().to_rfc3339(),
            "2026-07-29T06:00:00+00:00"
        );

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn history_window_still_sees_a_recent_run_past_the_row_cap() {
        let (pool, db) = setup_test_db().await;
        let t = "55555555-5555-5555-5555-555555555555";

        for i in 0..(RECENT_RUN_WINDOW + 10) {
            let ts = format!("2026-07-29T0{}:{:02}:00Z", 4 + i / 60, i % 60);
            seed_trigger_event(
                &pool,
                t,
                "TriggerExecuted",
                &ts,
                serde_json::json!({ "trigger_id": t, "last_run": ts }),
            )
            .await;
        }

        let history = load_trigger_run_history(&pool, t).await.unwrap();
        let newest = history.last_run.expect("some run must be reported");
        assert!(
            newest
                >= DateTime::parse_from_rfc3339("2026-07-29T04:59:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            "the cap must keep the newest rows, got {}",
            newest
        );

        teardown_test_db(&db).await;
    }
}
