use super::*;

impl EventStore {
    /// Get saved threads from the projection table.
    pub async fn get_saved_threads(
        &self,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.is_saved = TRUE ORDER BY t.last_activity DESC",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    /// Get recent threads for the drawer.
    ///
    /// Returns every `archive_state='inbox'` thread (the REVIEW pile) and the
    /// top `per_source` archived threads per source (the Archive pile). Inbox
    /// is unbounded by design: an inbox row is one the user hasn't dismissed,
    /// so capping it would silently hide work the user expects to see —
    /// crashed Claude Code sessions, idle chats they meant to come back to, and so on.
    /// Archive is capped because old archived threads aren't time-sensitive;
    /// the user can page back via `get_older_threads`.
    ///
    /// Also unconditionally includes active-status threads (`running`,
    /// `waiting_for_user_answer`) — a thread the user just started or that's
    /// blocked on user input must appear immediately, before any response
    /// arrives. And the actionable bypasses (`coding_agent_proposed=TRUE`,
    /// `status='failed'`, `status='waiting_for_user_answer'`) are preserved
    /// in case a future state lets one of those slip past `archive_state='inbox'`.
    pub async fn get_recent_threads(
        &self,
        per_source: i64,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let active_statuses = active_thread_statuses();
        let actionable_statuses: &[&str] = &[
            ThreadStatus::WaitingForUserAnswer.as_str(),
            ThreadStatus::Failed.as_str(),
        ];
        let sql = format!(
            "SELECT {} FROM (\
                SELECT *, ROW_NUMBER() OVER (PARTITION BY source ORDER BY last_activity DESC) AS rn \
                FROM thread_summaries \
                WHERE has_response = TRUE OR status = ANY($1) OR coding_agent_proposed = TRUE\
            ) t \
            WHERE t.archive_state = '{}' \
               OR t.rn <= $2 \
               OR t.coding_agent_proposed = TRUE \
               OR t.status = ANY($3) \
            ORDER BY t.last_activity DESC",
            THREAD_COLS.as_str(),
            ArchiveState::Inbox.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&active_statuses[..])
            .bind(per_source)
            .bind(actionable_statuses)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    /// Older threads for infinite scroll. When `trigger_ids`, `repo_ids`, or
    /// `app_ids` is provided the SQL collapses to a narrowing branch keyed on
    /// those columns plus the pagination cursor — the dropdown advertises every
    /// trigger / repo / app that ever stamped a row (no `has_response` /
    /// `source` gate), so the filter must match the same set or the dropdown
    /// lies. The three ID arrays compose with OR so a user can expand Trigger,
    /// Repos, and Apps in the dropdown and see all narrowings together. App ids
    /// are extracted from `coding_agent_folder` via `app_id_sql_expr` (the
    /// segment after `/data/apps/`). Without any of the three the normal history
    /// view applies (`has_response = TRUE` + optional channel filter).
    pub async fn get_older_threads(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        limit: i64,
        sources: Option<&[String]>,
        trigger_ids: Option<&[String]>,
        repo_ids: Option<&[String]>,
        app_ids: Option<&[String]>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = if trigger_ids.is_some() || repo_ids.is_some() || app_ids.is_some() {
            let trigger_ids_slice = trigger_ids.unwrap_or(&[]);
            let repo_ids_slice = repo_ids.unwrap_or(&[]);
            let app_ids_slice = app_ids.unwrap_or(&[]);
            let sql = format!(
                "SELECT {} FROM thread_summaries t \
                 WHERE (t.trigger_id = ANY($1) OR t.cc_repo_id = ANY($2) \
                        OR (t.coding_agent_kind = 'app' AND {APP_ID_EXPR} = ANY($3))) \
                   AND t.last_activity < $4 \
                 ORDER BY t.last_activity DESC LIMIT $5",
                THREAD_COLS.as_str(),
                APP_ID_EXPR = app_id_sql_expr("t"),
            );
            sqlx::query_as::<_, ThreadRow>(&sql)
                .bind(trigger_ids_slice)
                .bind(repo_ids_slice)
                .bind(app_ids_slice)
                .bind(before)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
        } else {
            let sql = format!(
                "SELECT {} FROM thread_summaries t \
                 WHERE t.has_response = TRUE AND t.last_activity < $1 \
                 AND ($2::text[] IS NULL OR t.source = ANY($2)) \
                 ORDER BY t.last_activity DESC LIMIT $3",
                THREAD_COLS.as_str(),
            );
            sqlx::query_as::<_, ThreadRow>(&sql)
                .bind(before)
                .bind(sources)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
        };

        Self::rows_to_thread_summaries(rows)
    }

    /// Distinct filter facets — every trigger / repo / app that has stamped at
    /// least one thread. Powers the drawer "Show" dropdown so it lists the
    /// COMPLETE set of selectable triggers/repos/apps (not just whatever is in
    /// the currently-loaded window). Labels are resolved client-side from the
    /// trigger / repository / app registries; this returns only ids +
    /// `last_activity` (newest thread per facet, for ordering deleted entries).
    pub async fn get_filter_facets(
        &self,
    ) -> Result<FilterFacets, Box<dyn std::error::Error + Send + Sync>> {
        let triggers = sqlx::query_as::<_, FilterFacet>(
            "SELECT trigger_id AS id, MAX(last_activity) AS last_activity \
             FROM thread_summaries \
             WHERE source = 'trigger' AND trigger_id IS NOT NULL \
             GROUP BY trigger_id",
        )
        .fetch_all(&self.pool)
        .await?;

        // No `source` gate: `cc_repo_id` is only ever set on claude_code
        // threads, so `cc_repo_id IS NOT NULL` already scopes it — and this
        // keeps the facet set identical to the `cc_repo_id = ANY(...)`
        // predicate in `get_older_threads`, which has no source gate either
        // (facet and filter must advertise the same set).
        let repos = sqlx::query_as::<_, FilterFacet>(
            "SELECT cc_repo_id AS id, MAX(last_activity) AS last_activity \
             FROM thread_summaries \
             WHERE cc_repo_id IS NOT NULL \
             GROUP BY cc_repo_id",
        )
        .fetch_all(&self.pool)
        .await?;

        // The `LIKE` guard drops any malformed `coding_agent_kind='app'` row
        // whose folder isn't under `data/apps/` — without it `split_part`
        // would yield an empty-string facet id.
        let apps_sql = format!(
            "SELECT {APP_ID_EXPR} AS id, MAX(last_activity) AS last_activity \
             FROM thread_summaries \
             WHERE coding_agent_kind = 'app' AND coding_agent_folder LIKE '%/data/apps/%' \
             GROUP BY {APP_ID_EXPR}",
            APP_ID_EXPR = app_id_sql_expr("thread_summaries"),
        );
        let apps = sqlx::query_as::<_, FilterFacet>(&apps_sql)
            .fetch_all(&self.pool)
            .await?;

        Ok(FilterFacets {
            triggers,
            repos,
            apps,
        })
    }

    /// Load every family member (ancestor + descendant via `parent_thread_id`)
    /// of the given base set that isn't already in the base. Drives the
    /// drawer's family-aware rendering: pagination is per-thread by
    /// `last_activity DESC`, but `ThreadDrawer.tsx → categorizeThreads`
    /// lifts a whole family up to the freshest member's recency. Without
    /// this helper, a family member whose own `last_activity` falls below
    /// the loaded window would silently vanish — the parent's badge would
    /// say "N/N done" but `nestByParent` would only render the in-window
    /// children. UNION (not UNION ALL) terminates the recursive walk even
    /// on a corrupted parent cycle; the single walk uses an OR join to
    /// climb to ancestors AND descend to children in one pass.
    ///
    /// `max_family` caps the result count after ORDER BY last_activity DESC —
    /// a workspace with a pathological fan-out (one root that spawned hundreds
    /// of sub-threads via triggers / Claude Code sessions) would otherwise pull every
    /// descendant into the initial /api/v1/threads payload, ballooning the
    /// frontend's threadMap and re-render cost. The newest members are kept;
    /// any visibly-old descendant that falls below the cap only matters when
    /// the user expands the family, at which point they can scroll-paginate
    /// down to it. Pass `i64::MAX` to opt out of the cap.
    pub async fn fetch_family_extension(
        &self,
        base_ids: &[String],
        max_family: i64,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        if base_ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "WITH RECURSIVE family AS ( \
                SELECT thread_id, parent_thread_id FROM thread_summaries \
                    WHERE thread_id::text = ANY($1) \
                UNION \
                SELECT t.thread_id, t.parent_thread_id FROM thread_summaries t \
                INNER JOIN family f \
                    ON t.thread_id = f.parent_thread_id \
                    OR t.parent_thread_id = f.thread_id \
            ) \
            SELECT {} FROM thread_summaries t \
            WHERE t.thread_id IN (SELECT thread_id FROM family) \
                AND t.thread_id::text != ALL($1) \
            ORDER BY t.last_activity DESC \
            LIMIT $2",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(base_ids)
            .bind(max_family)
            .fetch_all(&self.pool)
            .await?;
        Self::rows_to_thread_summaries(rows)
    }

    /// Threads in `composing` state — the new "drafts" surface. The sidebar
    /// renders these in the Drafts section. Returned newest-first so the row
    /// the user just touched is at the top.
    pub async fn get_composing_threads(
        &self,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t \
             WHERE t.state = 'composing' \
             ORDER BY t.last_activity DESC",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .fetch_all(&self.pool)
            .await?;
        Self::rows_to_thread_summaries(rows)
    }

    /// Distinct (trigger_id, trigger_name, last_activity) tuples from every
    /// thread ever spawned by a trigger. Name and last_activity are taken from
    /// the most-recent thread per trigger_id — `last_activity` lets the UI
    /// disambiguate when several deleted triggers share a name.
    pub async fn list_historical_triggers(
        &self,
    ) -> Result<
        Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let rows: Vec<(String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT DISTINCT ON (trigger_id) trigger_id, trigger_name, last_activity \
             FROM thread_summaries \
             WHERE trigger_id IS NOT NULL \
             ORDER BY trigger_id, last_activity DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Look up a thread's generated title from the projection. Returns None
    /// when the thread doesn't exist or hasn't been titled yet.
    pub async fn get_thread_title(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT title FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(t,)| t))
    }

    /// Check if a thread already has a generated title.
    pub async fn thread_has_title(
        &self,
        thread_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        Ok(self.get_thread_title(thread_uuid).await?.is_some())
    }

    /// Get thread info for specific thread IDs (used for active threads).
    pub async fn get_threads_by_ids(
        &self,
        thread_ids: &[String],
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        if thread_ids.is_empty() {
            return Ok(vec![]);
        }

        let uuids: Vec<uuid::Uuid> = thread_ids
            .iter()
            .filter_map(|s| uuid::Uuid::parse_str(s).ok())
            .collect();

        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.thread_id = ANY($1::uuid[])",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&uuids)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    /// Read-side helper for the script/trigger/LLM "list threads" surface.
    /// Returns newest-first by `last_activity`. `limit` is clamped by the
    /// caller (HTTP layer / LLM tool) to 1..=1000. See [`ThreadSummaryFilters`]
    /// for the `active` semantics.
    pub async fn list_thread_summaries(
        &self,
        filters: ThreadSummaryFilters<'_>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let active_statuses = active_thread_statuses();
        let sql = format!(
            "SELECT {} FROM thread_summaries t \
             WHERE ($1::bool IS NULL \
                    OR ($1 = TRUE AND t.status = ANY($2)) \
                    OR ($1 = FALSE AND NOT (t.status = ANY($2)))) \
               AND ($3::text[] IS NULL OR t.source = ANY($3)) \
             ORDER BY t.last_activity DESC LIMIT $4",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(filters.active)
            .bind(&active_statuses[..])
            .bind(filters.sources)
            .bind(filters.limit)
            .fetch_all(&self.pool)
            .await?;
        Self::rows_to_thread_summaries(rows)
    }

    /// Same filters as [`Self::list_thread_summaries`], but returns the
    /// count only. Cheaper than fetching N rows just to take `.len()` on
    /// big workspaces. Reuses [`ThreadSummaryFilters`] for signature
    /// symmetry with the list helper — `limit` is irrelevant for `COUNT(*)`
    /// and is ignored here.
    pub async fn count_thread_summaries(
        &self,
        filters: ThreadSummaryFilters<'_>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let active_statuses = active_thread_statuses();
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::bigint FROM thread_summaries t \
             WHERE ($1::bool IS NULL \
                    OR ($1 = TRUE AND t.status = ANY($2)) \
                    OR ($1 = FALSE AND NOT (t.status = ANY($2)))) \
               AND ($3::text[] IS NULL OR t.source = ANY($3))",
        )
        .bind(filters.active)
        .bind(&active_statuses[..])
        .bind(filters.sources)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }
}
