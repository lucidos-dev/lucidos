use super::*;

impl EventStore {
    /// Get saved threads from the projection table.
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

    /// Get recent threads for the drawer.
    ///
    /// Returns every `archive_state='inbox'` thread (the REVIEW pile) and the
    /// newest `archive_limit` archived threads (the Archive pile). Inbox
    /// is unbounded by design: an inbox row is one the user hasn't dismissed,
    /// so capping it would silently hide work the user expects to see —
    /// crashed coding-agent sessions, idle chats they meant to come back to, and so on.
    /// Archive is capped because old archived threads aren't time-sensitive;
    /// the user can page back via `get_older_threads`.
    ///
    /// The archive window is a SINGLE GLOBAL `created_at DESC` slice over archived
    /// rows — the newest `archive_limit` archived threads regardless of source — so
    /// it is a contiguous prefix of the archived-by-`created_at` list. This is the
    /// SAME axis AND granularity the drawer's Archive section sorts (`byCreated`)
    /// and `get_older_threads` pages by (one global `created_at` cursor), so the
    /// initial-window → first-scroll-page seam is gap-free. (The outer `ORDER BY` is
    /// irrelevant — the frontend re-sorts every section.)
    ///
    /// It is deliberately NOT per-source: a per-source window is a *union* of
    /// per-source `created_at` prefixes, which a single global cursor cannot page
    /// without gaps. When sources differ in density (many recent chats; sparse,
    /// older coding-agent / trigger threads) the per-source union dragged the cursor
    /// down to the sparse source's months-old boundary — surfacing coding-agent
    /// threads far above their real date, skipping every chat in between, and
    /// halting scroll early. Pure-chronological gives up the "newest-of-every-source
    /// up top" guarantee in the unfiltered view (old coding-agent threads sit at
    /// their true date, reachable by scroll or the coding-agent channel filter).
    ///
    /// The window ranks ONLY archived rows (`PARTITION BY (archive_state =
    /// 'archived')` splits the exactly-two-valued `ArchiveState` cleanly), so inbox
    /// rows never consume the archive budget.
    ///
    /// The outer `WHERE` has exactly two clauses — unbounded inbox + the contiguous
    /// archived window — and NO out-of-window bypass. An archived thread is NEVER
    /// injected ahead of its `created_at` position; archived `failed` /
    /// `waiting_for_user_answer` threads reach the drawer purely via
    /// `get_older_threads` pagination, at their true date (so the Archive pile stays
    /// gap-free). There is no `coding_agent_proposed` bypass because an
    /// archived-proposed thread is an impossible state: a thread with a pending
    /// in-workspace change has no Archive action (`is_blocking` → `[Discard, Apply]`
    /// only), and external-repo archiving emits `ChangeApplied` for each pending
    /// change before `ThreadArchived`, so the row is no longer proposed once
    /// archived. Active threads (`running` / `waiting_for_user_answer`) likewise
    /// surface via the unbounded inbox clause — they can't be archived while active.
    ///
    /// The INNER candidate filter (`has_response OR status = ANY($1) OR
    /// coding_agent_proposed`) is unrelated to the window: it scopes which rows are
    /// candidates at all (so an inbox proposed/active row with no response still
    /// surfaces) and mirrors the `has_response` gate in `get_older_threads`.
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
                WHERE has_response = TRUE OR status = ANY($1) OR coding_agent_proposed = TRUE\
            ) t \
            WHERE t.archive_state = '{inbox}' \
               OR (t.archive_state = '{archived}' AND t.rn <= $2) \
            ORDER BY t.last_user_action DESC",
            cols = THREAD_COLS.as_str(),
            archived = ArchiveState::Archived.as_str(),
            inbox = ArchiveState::Inbox.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&active_statuses[..])
            .bind(archive_limit)
            .fetch_all(&self.pool)
            .await?;

        Self::rows_to_thread_summaries(rows)
    }

    /// Older threads for infinite scroll, newest-first below the `before` cursor.
    /// `before` is a `created_at` timestamp — the drawer's Archive section sorts
    /// by creation time (matching the date each row displays), so the cursor must
    /// page through the same axis (the frontend sends the oldest loaded thread's
    /// `created_at`). Paging and display share one axis precisely so a
    /// recently-created-but-stale thread can't page in late and go missing from
    /// the top of the created-sorted list. (Saved sorts by `last_user_action`,
    /// but Saved is fully loaded via `get_saved_threads` — not paged here.) The
    /// channel / facet
    /// predicate is `channel_facet_filter_sql` — the exact
    /// mirror of the frontend `threadPassesChannelFilter`, so pagination surfaces
    /// precisely the rows the drawer shows (and `count_archived_threads` counts):
    /// a `sources` channel gate, then per-channel narrowing by `trigger_ids`
    /// (trigger rows) and `repo_ids`/`app_ids` (coding-agent rows, persisted as
    /// `claude_code`, unioned). Whole channels and a facet COMPOSE — a user can
    /// keep chat + coding-agent on AND
    /// sub-select one trigger and see the union. The old two-branch form got this
    /// wrong: any facet selection silently dropped the whole-channel rows.
    ///
    /// The substantive `has_response = TRUE` history gate is relaxed whenever ANY
    /// facet axis is sub-selected: the dropdown (`get_filter_facets`) advertises
    /// every trigger / repo / app that ever stamped a row with no `has_response`
    /// gate, so selecting one must surface its threads even if they never produced
    /// a response (a crashed coding-agent session, an errored trigger run) — else the
    /// dropdown lies. With no facet axis active this reduces to the plain
    /// `has_response = TRUE` channel view.
    pub async fn get_older_threads(
        &self,
        before: chrono::DateTime<chrono::Utc>,
        limit: i64,
        sources: Option<&[String]>,
        trigger_ids: Option<&[String]>,
        repo_ids: Option<&[String]>,
        app_ids: Option<&[String]>,
    ) -> Result<Vec<ThreadSummary>, Box<dyn std::error::Error + Send + Sync>> {
        // Binds: before($1), sources($2), trigger_ids($3), repo_ids($4),
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

    /// Distinct filter facets — every trigger / repo / app that has stamped at
    /// least one thread. Powers the drawer "Show" dropdown so it lists the
    /// COMPLETE set of selectable triggers/repos/apps (not just whatever is in
    /// the currently-loaded window). Labels are resolved client-side from the
    /// trigger / repository / app registries; this returns only ids +
    /// `last_activity` (newest thread per facet, for ordering deleted entries).
    pub async fn get_filter_facets(
        &self,
    ) -> Result<FilterFacets, Box<dyn std::error::Error + Send + Sync>> {
        // Trigger / app labels are resolved client-side, so `name` is NULL
        // here (the column must still exist for `FilterFacet`'s FromRow).
        let triggers = sqlx::query_as::<_, FilterFacet>(
            "SELECT trigger_id AS id, NULL::text AS name, MAX(last_activity) AS last_activity \
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
        //
        // Group first, then resolve `name` per distinct id in the outer query
        // (live registry → `repo_names` projection) so a removed repo lists
        // under its historical name instead of its UUID — keeping the resolution
        // out of the GROUP BY.
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

        // The `LIKE` guard drops any malformed `coding_agent_kind='app'` row
        // whose folder isn't under `data/apps/` — without it `split_part`
        // would yield an empty-string facet id.
        let apps_sql = format!(
            "SELECT {APP_ID_EXPR} AS id, NULL::text AS name, MAX(last_activity) AS last_activity \
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
    /// drawer's family-aware rendering: base Archive pagination is per-thread by
    /// `created_at DESC` (Saved is fully loaded), but `ThreadDrawer.tsx →
    /// nestByParent` renders a whole family together (and Saved lifts the family
    /// to its freshest member). Without this helper, a family member whose own
    /// `created_at` falls below the loaded window would silently vanish — the
    /// parent's badge would say "N/N done" but `nestByParent` would only render
    /// the in-window children. This extension is deliberately capped by
    /// `last_activity` (not `created_at`): an actively-streaming agent sub-thread
    /// created long ago must still be retained under the cap. UNION
    /// (not UNION ALL) terminates the recursive walk even
    /// on a corrupted parent cycle; the single walk uses an OR join to
    /// climb to ancestors AND descend to children in one pass.
    ///
    /// `max_family` caps the result count after ORDER BY last_activity DESC —
    /// a workspace with a pathological fan-out (one root that spawned hundreds
    /// of sub-threads via triggers / coding-agent sessions) would otherwise pull every
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

    /// Total threads in the Archive pile **matching the active drawer filter** —
    /// `archive_state='archived'` and not saved (a saved+archived thread routes
    /// to the Saved section, not Archive). Drives the collapsed Archive
    /// section's count badge, which would otherwise show only the loaded window
    /// (`get_recent_threads`'s `archive_limit` global slice + scroll-paginated
    /// rows) — a gross undercount on workspaces with hundreds of archived threads.
    ///
    /// The filter is `channel_facet_filter_sql`, shared verbatim with
    /// [`Self::get_older_threads`] so the badge total stays in lockstep with what
    /// scroll-pagination surfaces: a `sources` channel gate ANDed with per-channel
    /// facet narrowing (`trigger_ids` on trigger rows; `repo_ids`/`app_ids` on
    /// coding-agent rows, persisted as `claude_code`, unioned). Pass `None` for
    /// all four to count the whole pile. Whole channels and a facet COMPOSE —
    /// chat + coding-agent + one trigger counts every archived chat and
    /// coding-agent thread PLUS that trigger's, not
    /// the trigger's alone (the pre-fix bug, where the facet branch ignored
    /// `sources`). A server-sourced count is what keeps the badge stable: it does
    /// not drift as rows page in or as the section is collapsed/expanded.
    ///
    /// Intentionally a flat count of the archived pile, NOT a per-thread replay
    /// of `displaySection`'s family routing. The handful of edge cases it
    /// glosses over (an archived thread with a still-active descendant routes
    /// to Review/Active) move at most a few rows, and the badge is a
    /// collapsed-section indicator that is never rendered alongside the actual
    /// rows — so duplicating the routing logic in SQL isn't worth the drift
    /// against the `thread_lifecycle.rs` source of truth.
    pub async fn count_archived_threads(
        &self,
        sources: Option<&[String]>,
        trigger_ids: Option<&[String]>,
        repo_ids: Option<&[String]>,
        app_ids: Option<&[String]>,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let archived = ArchiveState::Archived.as_str();
        // Binds: archived($1), sources($2), trigger_ids($3), repo_ids($4),
        // app_ids($5) — facet positions 2..=5 match the shared filter helper.
        let sql = format!(
            "SELECT COUNT(*)::bigint FROM thread_summaries t \
             WHERE t.archive_state = $1 AND t.is_saved = FALSE \
               AND {filter}",
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
