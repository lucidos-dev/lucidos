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

    /// Recover a `repo_names` entry for repos that predate the `RepositoryAdded`
    /// event, so the filter / thread rows stop showing their raw UUID.
    ///
    /// Why: the `repo_names` projection (migration `20260614162518`) and its
    /// initial backfill draw the name from the live `repositories` registry and
    /// the `RepositoryAdded` event log. But a repo added before that event was
    /// wired up emits no `RepositoryAdded`, and once removed (`DELETE FROM
    /// repositories`) it's absent from the registry too — so neither source
    /// knows its name and the read path falls back to the UUID. The repo's
    /// *path*, however, still lives in `changes.repo_root` (recorded per
    /// proposed change), and a repo's name is conventionally the path's final
    /// segment. This scavenges that basename as a last-resort name.
    ///
    /// `ON CONFLICT DO NOTHING` keeps the authoritative names (registry / event)
    /// from the initial backfill untouched — this only fills genuine gaps. App
    /// coding-agent threads record a *workspace* root in `repo_root` (whose
    /// basename is the workspace, not a repo), so they're excluded. Once-only —
    /// guarded by a marker; going forward `RepositoryAdded` keeps `repo_names`
    /// current with the real user-chosen name.
    pub async fn backfill_repo_names_from_changes(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::PreferenceStore::get(&self.pool, BACKFILL_REPO_NAMES_FROM_CHANGES_MARKER)
            .await?
            .is_some()
        {
            return Ok(0);
        }

        // Per repo, take the most-recent change's `repo_root` and reduce it to
        // its final path segment (`^.*/` strips everything up to the last
        // separator; `rtrim` drops a stray trailing slash first). The regex
        // guards the `::uuid` cast against any malformed `cc_repo_id`.
        let inserted = sqlx::query(
            "INSERT INTO repo_names (id, name) \
             SELECT DISTINCT ON (ts.cc_repo_id) \
                 ts.cc_repo_id::uuid AS id, \
                 regexp_replace(rtrim(c.repo_root, '/'), '^.*/', '') AS name \
             FROM thread_summaries ts \
             JOIN changes c ON c.thread_id = ts.thread_id \
             WHERE ts.cc_repo_id IS NOT NULL \
               AND ts.cc_repo_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' \
               AND ts.coding_agent_kind IS DISTINCT FROM 'app' \
               AND c.repo_root IS NOT NULL \
               AND regexp_replace(rtrim(c.repo_root, '/'), '^.*/', '') <> '' \
             ORDER BY ts.cc_repo_id, c.created_at DESC \
             ON CONFLICT (id) DO NOTHING",
        )
        .execute(&self.pool)
        .await?
        .rows_affected() as usize;

        crate::core::PreferenceStore::set(
            &self.pool,
            BACKFILL_REPO_NAMES_FROM_CHANGES_MARKER,
            "1",
        )
        .await?;
        Ok(inserted)
    }

    /// Re-point coding-agent threads orphaned by the old random-UUID
    /// `repositories` registry onto the default Lucidos repo's **deterministic**
    /// id (`uuidv5(namespace, root_commit_sha)`), passed in by the caller.
    ///
    /// Why: `repositories.id` used to be a random `gen_random_uuid()` regenerated
    /// on every remove+re-add / registry wipe / directory move, so a thread bound
    /// at first `SessionStarted` to a prior id was orphaned — the frontend repo
    /// filter keys live-vs-`(deleted)` on id (`repoFilters.ts`), so it showed a
    /// "(deleted)"/missing-repo badge even though the checkout was present. With
    /// deterministic ids this can't recur, so this is a ONE-TIME cleanup, not a
    /// permanent alias layer.
    ///
    /// General and per-workspace-correct — NO hardcoded UUIDs, NO name
    /// heuristics: a *Lucidos-source* coding-agent thread targets the Lucidos
    /// repo *by definition*, so its `cc_repo_id` is `default_repo_det_id`. The
    /// WHERE clause identifies Lucidos-source threads conservatively:
    ///  - `coding_agent_is_external_repo = FALSE` — the durable external marker
    ///    (a NOT-NULL bool predating `coding_agent_kind`), so a **legacy
    ///    external-repo thread** (created before the kind column, hence
    ///    `coding_agent_kind IS NULL` but flagged external) is NOT mis-repointed;
    ///  - `coding_agent_kind = 'lucidos' OR NULL` — excludes `'app'` and modern
    ///    `'external'` threads;
    ///  - `cc_repo_id` is NOT a currently-registered repository — a live binding
    ///    isn't orphaned (no badge) and must not be rewritten; this also shields
    ///    a still-registered external repo whose earliest threads predate the
    ///    external flag column.
    /// App threads (kind `'app'`, NULL `cc_repo_id`) are untouched. Once-only —
    /// guarded by a marker.
    pub async fn backfill_cc_repo_id_to_deterministic(
        &self,
        default_repo_det_id: uuid::Uuid,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        if crate::core::PreferenceStore::get(
            &self.pool,
            BACKFILL_CC_REPO_ID_DETERMINISTIC_MARKER,
        )
        .await?
        .is_some()
        {
            return Ok(0);
        }

        let det = default_repo_det_id.to_string();
        let updated = sqlx::query(
            "UPDATE thread_summaries ts \
             SET cc_repo_id = $1 \
             WHERE ts.is_coding_agent \
               AND ts.cc_repo_id IS NOT NULL \
               AND ts.cc_repo_id <> $1 \
               AND ts.coding_agent_is_external_repo = FALSE \
               AND (ts.coding_agent_kind = 'lucidos' OR ts.coding_agent_kind IS NULL) \
               AND NOT EXISTS (SELECT 1 FROM repositories r WHERE r.id::text = ts.cc_repo_id)",
        )
        .bind(&det)
        .execute(&self.pool)
        .await?
        .rows_affected() as usize;

        crate::core::PreferenceStore::set(
            &self.pool,
            BACKFILL_CC_REPO_ID_DETERMINISTIC_MARKER,
            "1",
        )
        .await?;
        Ok(updated)
    }
}
