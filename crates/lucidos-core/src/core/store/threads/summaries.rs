use super::*;

impl EventStore {
    pub async fn get_saved_threads(
        &self,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.is_saved = TRUE ORDER BY t.last_user_action DESC",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    pub async fn get_recent_threads(
        &self,
        archive_limit: i64,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let active_statuses = active_thread_statuses();
        let sql = format!(
            "SELECT {cols} FROM (\
                SELECT *, ROW_NUMBER() OVER (\
                    PARTITION BY (archive_state = '{archived}') ORDER BY created_at DESC\
                ) AS rn \
                FROM thread_summaries \
                WHERE has_response = TRUE OR status = ANY($1) OR coding_agent_change_state = '{proposed}'\
            ) t \
            WHERE (t.archive_state = '{inbox}' \
               OR (t.archive_state = '{archived}' AND t.rn <= $2)) \
            ORDER BY t.last_user_action DESC",
            cols = THREAD_COLS.as_str(),
            archived = ArchiveState::Archived.as_str(),
            inbox = ArchiveState::Inbox.as_str(),
            proposed = crate::engine::thread_lifecycle::ChangeStateKind::Proposed.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&active_statuses[..])
            .bind(archive_limit)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    pub async fn get_older_threads(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        limit: i64,
        sources: Option<&[String]>,
        trigger_ids: Option<&[String]>,
        repo_ids: Option<&[String]>,
        app_ids: Option<&[String]>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Binds: before($1), sources($2), trigger_ids($3), repo_ids($4),
        // app_ids($5), limit($6) — the facet positions 2..=5 match the shared
        // filter helper and the has_response relaxation below.
        let sql = format!(
            "SELECT {cols} FROM thread_summaries t \
             WHERE t.created_at < $1 \
               AND (t.has_response = TRUE \
                    OR array_length($3::text[], 1) IS NOT NULL \
                    OR array_length($4::text[], 1) IS NOT NULL \
                    OR array_length($5::text[], 1) IS NOT NULL) \
               AND {filter} \
             ORDER BY t.created_at DESC LIMIT $6",
            cols = THREAD_COLS.as_str(),
            filter = channel_facet_filter_sql("t", 2, 3, 4, 5),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(before)
            .bind(sources)
            .bind(trigger_ids)
            .bind(repo_ids)
            .bind(app_ids)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    pub async fn get_filter_facets(
        &self,
        apps: &crate::core::AppManager,
    ) -> Result<FilterFacets, Box<dyn std::error::Error + Send + Sync>> {
        let triggers = sqlx::query_as::<_, FilterFacet>(
            "SELECT trigger_id AS id, NULL::text AS name, MAX(last_activity) AS last_activity \
             FROM thread_summaries \
             WHERE source = 'trigger' AND trigger_id IS NOT NULL \
             GROUP BY trigger_id",
        )
        .fetch_all(&self.pool)
        .await?;

        let repos_sql = format!(
            "SELECT sub.id AS id, {repo_name} AS name, sub.last_activity AS last_activity \
             FROM ( \
                SELECT cc_repo_id AS id, MAX(last_activity) AS last_activity \
                FROM thread_summaries \
                WHERE cc_repo_id IS NOT NULL \
                GROUP BY cc_repo_id \
             ) sub",
            repo_name = repo_name_expr("sub.id"),
        );
        let repos = sqlx::query_as::<_, FilterFacet>(&repos_sql)
            .fetch_all(&self.pool)
            .await?;

        // INVARIANT: The `LIKE` guard drops any malformed `coding_agent_kind='app'` row
        // whose folder isn't under `data/apps/` — without it `split_part`
        // would yield an empty-string facet id.
        let apps_sql = format!(
            "SELECT sub.id, k.kind AS recorded_kind, sub.last_activity \
             FROM ( \
                SELECT {APP_ID_EXPR} AS id, MAX(last_activity) AS last_activity \
                FROM thread_summaries \
                WHERE coding_agent_kind = 'app' AND coding_agent_folder LIKE '%/data/apps/%' \
                GROUP BY {APP_ID_EXPR} \
             ) sub \
             LEFT JOIN app_kinds k ON k.app_id = sub.id",
            APP_ID_EXPR = app_id_sql_expr("thread_summaries"),
        );
        let rows = sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<chrono::DateTime<chrono::Utc>>,
            ),
        >(&apps_sql)
        .fetch_all(&self.pool)
        .await?;
        let apps = rows
            .into_iter()
            .map(|(id, recorded_kind, last_activity)| {
                app_filter_facet(apps, id, recorded_kind.as_deref(), last_activity)
            })
            .collect();

        Ok(FilterFacets {
            triggers,
            repos,
            apps,
        })
    }

    pub async fn fetch_family_extension(
        &self,
        base_ids: &[String],
        max_family: i64,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let base_uuids: Vec<uuid::Uuid> = base_ids
            .iter()
            .filter_map(|id| {
                let uuid = uuid::Uuid::parse_str(id).ok()?;
                let canonical = uuid
                    .as_hyphenated()
                    .encode_lower(&mut uuid::Uuid::encode_buffer())
                    == id;
                canonical.then_some(uuid)
            })
            .collect();
        if base_uuids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "WITH RECURSIVE family AS ( \
                SELECT thread_id, parent_thread_id FROM thread_summaries \
                    WHERE thread_id = ANY($1) \
                UNION \
                SELECT t.thread_id, t.parent_thread_id FROM thread_summaries t \
                INNER JOIN family f \
                    ON t.thread_id = f.parent_thread_id \
                    OR t.parent_thread_id = f.thread_id \
            ) \
            SELECT {} FROM thread_summaries t \
            WHERE t.thread_id IN (SELECT thread_id FROM family) \
                AND t.thread_id != ALL($1) \
            ORDER BY t.last_activity DESC \
            LIMIT $2",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&base_uuids)
            .bind(max_family)
            .fetch_all(&self.pool)
            .await?;
        Self::rows_to_thread_summaries(rows)
    }

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

    /// INVARIANT: Titles of the threads whose agent is waiting on the user, newest first.
    ///
    /// Titles only, and never the questions themselves. This answers "is
    /// anything waiting on me?" across the workspace, and a *voice session*
    /// asks it while somebody holds a phone to their ear.
    ///
    /// Read off the projected status rather than swept out of the event log.
    /// One row per thread against a table of thousands, versus a correlated
    /// scan of every question ever asked.
    ///
    /// **Two axes, and BOTH are needed.** `state` covers a discarded or
    /// still-composing draft. `archive_state` covers an archived thread, which
    /// keeps `state = 'active'` on purpose, so a `state` filter alone lets
    /// every archived thread through. Either way the user put the thread down,
    /// and a card still open on it is waiting on nobody.
    pub async fn titles_awaiting_answer(
        &self,
        limit: i64,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let rows: Vec<(Option<String>,)> = sqlx::query_as(&format!(
            "SELECT title FROM thread_summaries \
             WHERE status = {} \
               AND state = 'active' AND archive_state = 'inbox' \
             ORDER BY last_activity DESC LIMIT $1",
            ThreadStatus::WaitingForUserAnswer.sql_literal(),
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(title,)| match title {
                Some(title) if !title.trim().is_empty() => title,
                // INVARIANT: A thread whose title has not been generated yet is still
                // waiting. Dropping it would report "nothing needs you".
                _ => UNTITLED_THREAD.to_string(),
            })
            .collect())
    }

    pub async fn thread_has_title(
        &self,
        thread_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let thread_uuid = uuid::Uuid::parse_str(thread_id)?;
        Ok(self.get_thread_title(thread_uuid).await?.is_some())
    }

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

    pub async fn list_thread_summaries(
        &self,
        filters: ThreadSummaryFilters<'_>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        let (statuses, negate) = filters.status.binds();
        let sql = format!(
            "SELECT {cols} FROM thread_summaries t \
             WHERE {status} AND {filters} \
             ORDER BY t.last_activity DESC LIMIT $7",
            cols = THREAD_COLS.as_str(),
            status = STATUS_FILTER_SQL.as_str(),
            filters = SUMMARY_FILTER_SQL.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(statuses)
            .bind(negate)
            .bind(filters.sources)
            .bind(filters.parent)
            .bind(filters.has_draft)
            .bind(filters.change_state.map(ChangeStateKind::as_str))
            .bind(filters.limit)
            .fetch_all(&self.pool)
            .await?;
        let mut summaries = Self::rows_to_thread_summaries(rows)?;
        // INVARIANT: A parent reads its children here, so each row says what waits below
        // it. Without this an orchestrator's row shows nothing pending while
        // its children hold every change.
        let ids: Vec<uuid::Uuid> = summaries
            .iter()
            .filter_map(|s| uuid::Uuid::parse_str(&s.thread_id).ok())
            .collect();
        let counts =
            crate::core::changes::pending_sub_thread_change_counts(&self.pool, &ids).await?;
        for summary in &mut summaries {
            let count = uuid::Uuid::parse_str(&summary.thread_id)
                .ok()
                .and_then(|id| counts.get(&id).copied())
                .unwrap_or(0);
            summary.pending_sub_thread_change_count = Some(count);
        }
        Ok(summaries)
    }

    /// INVARIANT: Same filters as [`Self::list_thread_summaries`], but returns the
    /// count only. Cheaper than fetching N rows just to take `.len()` on
    /// big workspaces. Reuses [`ThreadSummaryFilters`] for signature
    /// symmetry with the list helper: `limit` is irrelevant for `COUNT(*)`
    /// and is ignored here.
    ///
    /// The predicates are [`STATUS_FILTER_SQL`] and [`SUMMARY_FILTER_SQL`],
    /// shared verbatim with the list query, so a count can never disagree with
    /// the rows it is counting.
    pub async fn count_thread_summaries(
        &self,
        filters: ThreadSummaryFilters<'_>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let (statuses, negate) = filters.status.binds();
        let sql = format!(
            "SELECT COUNT(*)::bigint FROM thread_summaries t WHERE {status} AND {filters}",
            status = STATUS_FILTER_SQL.as_str(),
            filters = SUMMARY_FILTER_SQL.as_str(),
        );
        let (count,): (i64,) = sqlx::query_as(&sql)
            .bind(statuses)
            .bind(negate)
            .bind(filters.sources)
            .bind(filters.parent)
            .bind(filters.has_draft)
            .bind(filters.change_state.map(ChangeStateKind::as_str))
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    pub async fn count_archived_threads(
        &self,
        sources: Option<&[String]>,
        trigger_ids: Option<&[String]>,
        repo_ids: Option<&[String]>,
        app_ids: Option<&[String]>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let archived = ArchiveState::Archived.as_str();
        // INVARIANT: Binds: archived($1), sources($2), trigger_ids($3), repo_ids($4),
        // app_ids($5) — facet positions 2..=5 match the shared filter helper.
        let sql = format!(
            "SELECT COUNT(*)::bigint FROM thread_summaries t \
             WHERE t.archive_state = $1 AND {filter}",
            filter = channel_facet_filter_sql("t", 2, 3, 4, 5),
        );
        let (count,): (i64,) = sqlx::query_as(&sql)
            .bind(archived)
            .bind(sources)
            .bind(trigger_ids)
            .bind(repo_ids)
            .bind(app_ids)
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }
}

fn app_filter_facet(
    apps: &crate::core::AppManager,
    id: String,
    recorded_kind: Option<&str>,
    last_activity: Option<chrono::DateTime<chrono::Utc>>,
) -> AppFilterFacet {
    if !apps.app_exists(&id) {
        let kind = recorded_kind
            .and_then(|k| serde_json::from_value(serde_json::Value::String(k.to_string())).ok());
        return AppFilterFacet {
            id,
            name: None,
            kind,
            deleted: true,
            last_activity,
        };
    }
    let app = apps.get_app(&id).ok();
    AppFilterFacet {
        name: app.as_ref().map(|a| a.name.clone()),
        kind: app.map(|a| a.kind),
        id,
        deleted: false,
        last_activity,
    }
}
