use super::*;

impl EventStore {
    /// Recover `trigger_id`/`trigger_name` for trigger-source rows the
    /// `20260429214800_addtriggeridtothreadsummaries.sql` migration left NULL.
    ///
    /// Why: the SQL backfill in that migration only reads `payload->>'trigger_id'`,
    /// but legacy `TriggerStarted` events store the value under `task_id`
    /// (Rust deserializes both via `#[serde(alias = "task_id")]`, but Postgres
    /// doesn't honor serde aliases). Result: every pre-rename trigger thread
    /// shipped with NULL `trigger_id`, so the dropdown filter matched nothing
    /// and the historical-triggers list was empty.
    ///
    /// This function COALESCEs both shapes and writes the *first* TriggerStarted
    /// event's id/name per thread — same "first write wins" semantics as the
    /// runtime projection in `event_bus::apply_thread_event`. Runs before
    /// `backfill_trigger_id_v5_to_config_id` so the v5→config_id rewrite picks
    /// up the freshly recovered rows. Once-only — guarded by a marker.
    pub async fn backfill_trigger_id_from_events(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::PreferenceStore::get(&self.pool, BACKFILL_TRIGGER_ID_FROM_EVENTS_MARKER)
            .await?
            .is_some()
        {
            return Ok(0);
        }

        let updated = sqlx::query(
            "UPDATE thread_summaries ts \
             SET trigger_id = first_evt.trigger_id, \
                 trigger_name = first_evt.trigger_name \
             FROM ( \
                 SELECT DISTINCT ON (e.aggregate_id) \
                     e.aggregate_id, \
                     COALESCE(e.payload->>'trigger_id', e.payload->>'task_id')     AS trigger_id, \
                     COALESCE(e.payload->>'trigger_name', e.payload->>'task_name') AS trigger_name \
                 FROM events e \
                 WHERE e.event_type = 'TriggerStarted' \
                 ORDER BY e.aggregate_id, e.created ASC \
             ) AS first_evt \
             WHERE first_evt.aggregate_id = ts.thread_id::text \
               AND ts.source = 'trigger' \
               AND ts.trigger_id IS NULL \
               AND first_evt.trigger_id IS NOT NULL",
        )
        .execute(&self.pool)
        .await?
        .rows_affected() as usize;

        crate::core::PreferenceStore::set(
            &self.pool,
            BACKFILL_TRIGGER_ID_FROM_EVENTS_MARKER,
            "1",
        )
        .await?;
        Ok(updated)
    }

    /// Rewrite legacy `thread_summaries.trigger_id` rows that hold the v5
    /// task UUID (`trigger_id_to_uuid(config.id)`) back to the raw `config.id`.
    ///
    /// Why: pre-fix, the scheduler passed the v5 task UUID into TriggerStarted,
    /// and the projection persisted it. The dropdown filter sends `config.id`
    /// (from `/api/v1/triggers`), so the SQL filter never matched and live
    /// triggers showed zero threads.
    ///
    /// Once-only — guarded by a marker in `preferences` so subsequent boots
    /// don't re-scan `events`. Builds the {v5_hash → config.id} map from
    /// `TriggerCreated` events and rewrites matching rows in a single UPDATE.
    /// Returns the number of rows updated (0 on subsequent runs).
    pub async fn backfill_trigger_id_v5_to_config_id(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::PreferenceStore::get(&self.pool, BACKFILL_TRIGGER_ID_V5_MARKER)
            .await?
            .is_some()
        {
            return Ok(0);
        }

        let config_ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT payload->>'trigger_id' FROM events \
             WHERE event_type = 'TriggerCreated' AND payload->>'trigger_id' IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut v5_hashes: Vec<String> = Vec::with_capacity(config_ids.len());
        let mut config_ids_paired: Vec<String> = Vec::with_capacity(config_ids.len());
        for cid in config_ids {
            let v5 = crate::scheduler::trigger_id_to_uuid(&cid).to_string();
            if v5 != cid {
                v5_hashes.push(v5);
                config_ids_paired.push(cid);
            }
        }

        let updated = if v5_hashes.is_empty() {
            0
        } else {
            sqlx::query(
                "UPDATE thread_summaries ts \
                 SET trigger_id = m.config_id \
                 FROM (SELECT unnest($1::text[]) AS v5, unnest($2::text[]) AS config_id) AS m \
                 WHERE ts.trigger_id = m.v5",
            )
            .bind(&v5_hashes)
            .bind(&config_ids_paired)
            .execute(&self.pool)
            .await?
            .rows_affected() as usize
        };

        crate::core::PreferenceStore::set(&self.pool, BACKFILL_TRIGGER_ID_V5_MARKER, "1").await?;
        Ok(updated)
    }
}
