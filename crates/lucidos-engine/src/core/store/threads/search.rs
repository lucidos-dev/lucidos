use super::*;

impl EventStore {
    /// Search threads by text query (ILIKE on titles and message content).
    /// Multi-token queries match per-token: every whitespace-separated token must
    /// appear (case-insensitive) somewhere in the thread — title or any event
    /// payload — but they need not appear together as a phrase. Title-only
    /// matches score 1.0; content matches score 0.7.
    pub async fn search_threads_by_text(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        // Escape LIKE metacharacters (\ % _) so a token like "50%" matches the
        // literal substring, not "anything containing 50". Backslash is LIKE's
        // default escape character, so no ESCAPE clause is needed.
        let patterns: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("%{}%", super::super::escape_like(t)))
            .collect();
        if patterns.is_empty() {
            return Ok(vec![]);
        }
        let token_count = patterns.len() as i64;

        // Search query joins `best_scores b` against `thread_summaries s`, so
        // the FROM alias is `s` instead of the default `t`. SearchRow below
        // reads `is_saved` but ignores `has_response`, the one column the shared
        // helper emits that a search result has no use for: the extra DB byte
        // per row is cheaper than a parallel copy of the column list.
        let thread_cols_prefixed = thread_cols("s");

        let sql = format!(
            "WITH title_matches AS (\
                SELECT thread_id, 1.0::float8 AS match_score FROM thread_summaries WHERE title ILIKE ALL($1::text[])\
            ), content_matches AS (\
                SELECT m.thread_id, 0.7::float8 AS match_score FROM (\
                    SELECT e.thread_id, t.pattern \
                    FROM events e CROSS JOIN unnest($1::text[]) AS t(pattern) \
                    WHERE e.event_type IN ('MessageReceived', 'ResponseGenerated') \
                      AND e.thread_id IS NOT NULL \
                      AND (e.payload->>'text' ILIKE t.pattern OR e.payload->>'content' ILIKE t.pattern)\
                ) m \
                GROUP BY m.thread_id \
                HAVING COUNT(DISTINCT m.pattern) = $3\
            ), entity_matches AS (\
                /* Drive from memory_entries (the small side) and join into events; \
                   joining the other way produces a ~58M-row nested loop on a large workspace. */\
                SELECT m.thread_id, 0.7::float8 AS match_score FROM (\
                    SELECT e.thread_id, t.pattern \
                    FROM memory_entries me \
                    JOIN events e ON e.id::text = me.source->>'id' \
                    CROSS JOIN unnest($1::text[]) AS t(pattern) \
                    WHERE me.source->>'type' = 'event' \
                      AND e.event_type IN ('MessageReceived', 'ResponseGenerated') \
                      AND e.thread_id IS NOT NULL \
                      AND EXISTS (\
                          SELECT 1 FROM jsonb_array_elements_text(me.entities) AS ent \
                          WHERE ent ILIKE t.pattern\
                      )\
                ) m \
                GROUP BY m.thread_id \
                HAVING COUNT(DISTINCT m.pattern) = $3\
            ), scored AS (\
                SELECT thread_id, match_score FROM title_matches \
                UNION ALL \
                SELECT thread_id, match_score FROM content_matches \
                UNION ALL \
                SELECT thread_id, match_score FROM entity_matches\
            ), best_scores AS (\
                SELECT thread_id, MAX(match_score) AS score FROM scored GROUP BY thread_id\
            ) \
            SELECT {}, b.score \
            FROM best_scores b JOIN thread_summaries s ON s.thread_id = b.thread_id \
            ORDER BY b.score DESC, s.last_activity DESC LIMIT $2",
            thread_cols_prefixed,
        );

        // Row type: ThreadRow fields + score. Struct needed because >16 columns exceeds sqlx tuple limit.
        #[derive(sqlx::FromRow)]
        struct SearchRow {
            thread_id: String,
            title: Option<String>,
            first_message: Option<String>,
            source: String,
            initiator: String,
            created_at: chrono::DateTime<chrono::Utc>,
            last_activity: chrono::DateTime<chrono::Utc>,
            last_user_action: chrono::DateTime<chrono::Utc>,
            last_agent_action: chrono::DateTime<chrono::Utc>,
            message_count: i64,
            is_saved: bool,
            section: String,
            active_children_count: i64,
            total_children_count: i64,
            blocking_descendant_count: i64,
            attention_descendant_count: i64,
            status: String,
            coding_agent_has_diff: bool,
            coding_agent_proposed: bool,
            coding_agent_requires_restart: bool,
            coding_agent_is_external_repo: bool,
            coding_agent_applying: bool,
            last_revived_at: Option<chrono::DateTime<chrono::Utc>>,
            parent_thread_id: Option<String>,
            parent_thread_title: Option<String>,
            trigger_id: Option<String>,
            trigger_name: Option<String>,
            cc_repo_id: Option<String>,
            cc_repo_name: Option<String>,
            coding_agent_kind: Option<String>,
            coding_agent_folder: Option<String>,
            coding_agent: Option<String>,
            state: String,
            compose_text: String,
            compose_images: serde_json::Value,
            compose_mode: Option<String>,
            compose_selection: Option<serde_json::Value>,
            compose_epoch: i64,
            score: f64,
        }

        let rows = sqlx::query_as::<_, SearchRow>(&sql)
            .bind(&patterns)
            .bind(limit)
            .bind(token_count)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|r| {
                Ok(ThreadSearchResult {
                    info: ThreadSummary {
                        thread_id: r.thread_id,
                        title: format_display_title(r.title, r.first_message),
                        channel: r.source,
                        initiator: LegacyInitiator::from_db_str(r.initiator.as_str())?,
                        created_at: r.created_at,
                        last_activity: r.last_activity,
                        last_user_action: r.last_user_action,
                        last_agent_action: r.last_agent_action,
                        message_count: r.message_count,
                        saved: r.is_saved,
                        section: r.section,
                        active_children_count: r.active_children_count,
                        total_children_count: r.total_children_count,
                        blocking_descendant_count: r.blocking_descendant_count,
                        attention_descendant_count: r.attention_descendant_count,
                        status: r.status,
                        coding_agent_has_diff: r.coding_agent_has_diff,
                        coding_agent_proposed: r.coding_agent_proposed,
                        coding_agent_requires_restart: r.coding_agent_requires_restart,
                        coding_agent_is_external_repo: r.coding_agent_is_external_repo,
                        coding_agent_applying: r.coding_agent_applying,
                        last_revived_at: r.last_revived_at,
                        parent_thread_id: r.parent_thread_id,
                        parent_thread_title: r.parent_thread_title,
                        trigger_id: r.trigger_id,
                        trigger_name: r.trigger_name,
                        cc_repo_id: r.cc_repo_id,
                        cc_repo_name: r.cc_repo_name,
                        coding_agent_kind: r.coding_agent_kind,
                        coding_agent_folder: r.coding_agent_folder,
                        coding_agent: r.coding_agent,
                        state: r.state,
                        compose_text: r.compose_text,
                        compose_images: r.compose_images,
                        compose_mode: r.compose_mode,
                        compose_selection: r.compose_selection,
                        compose_epoch: r.compose_epoch,
                    },
                    score: r.score,
                })
            })
            .collect()
    }

    /// Search threads semantically using memory_entries vector search.
    /// Accepts event IDs paired with their similarity scores.
    pub async fn search_threads_by_memory(
        &self,
        scored_event_ids: &[(uuid::Uuid, f64)],
        limit: i64,
    ) -> Result<Vec<ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        if scored_event_ids.is_empty() {
            return Ok(vec![]);
        }

        let event_ids: Vec<uuid::Uuid> = scored_event_ids.iter().map(|(id, _)| *id).collect();

        // Build a map of event_id → best similarity score
        let mut event_scores: std::collections::HashMap<uuid::Uuid, f64> =
            std::collections::HashMap::new();
        for (id, score) in scored_event_ids {
            let entry = event_scores.entry(*id).or_insert(0.0);
            if *score > *entry {
                *entry = *score;
            }
        }

        // Find which thread each event belongs to
        let thread_event_rows = sqlx::query_as::<_, (String, uuid::Uuid)>(
            r#"
            SELECT DISTINCT thread_id::text, id
            FROM events
            WHERE id = ANY($1::uuid[])
              AND thread_id IS NOT NULL
            "#,
        )
        .bind(&event_ids)
        .fetch_all(&self.pool)
        .await?;

        // Build thread_id → best score
        let mut thread_scores: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for (thread_id, event_id) in &thread_event_rows {
            if let Some(&score) = event_scores.get(event_id) {
                let entry = thread_scores.entry(thread_id.clone()).or_insert(0.0);
                if score > *entry {
                    *entry = score;
                }
            }
        }

        let thread_uuids: Vec<uuid::Uuid> = thread_scores
            .keys()
            .filter_map(|id| uuid::Uuid::parse_str(id).ok())
            .collect();

        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.thread_id = ANY($1::uuid[]) LIMIT $2",
            THREAD_COLS.as_str(),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&thread_uuids)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        let infos = Self::rows_to_thread_summaries(rows)?;
        let mut results: Vec<ThreadSearchResult> = infos
            .into_iter()
            .map(|info| {
                let score = thread_scores.get(&info.thread_id).copied().unwrap_or(0.5);
                ThreadSearchResult { info, score }
            })
            .collect();
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }
}
