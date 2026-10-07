use super::*;

/// The text of a message event, exactly as `idx_events_message_text_trgm`
/// indexes it. Any difference and the planner cannot use the index.
const MESSAGE_TEXT: &str =
    "(COALESCE(e.payload->>'text', '') || ' ' || COALESCE(e.payload->>'content', ''))";

/// The predicate of both partial message indexes, verbatim.
const IS_MESSAGE: &str =
    "e.event_type IN ('MessageReceived', 'ResponseGenerated') AND e.thread_id IS NOT NULL";

/// A trigram index needs 3 characters to narrow a search. A shorter token
/// would make Postgres recheck every message.
const MIN_INDEXED_TOKEN_CHARS: usize = 3;

fn is_indexable(token: &str) -> bool {
    token.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_INDEXED_TOKEN_CHARS
}

/// Escape LIKE metacharacters (\ % _) so a token like "50%" matches the
/// literal substring, not "anything containing 50". Backslash is LIKE's
/// default escape character, so no ESCAPE clause is needed.
fn contains_pattern(token: &str) -> String {
    format!("%{}%", super::super::escape_like(token))
}

/// Binds, in order:
/// - $1 every token's pattern
/// - $2 the limit
/// - $3 the token count
/// - $4 the query as a phrase pattern
/// - $5 the query as an exact pattern
/// - $6 the long tokens' patterns
/// - $7 the short tokens' patterns
pub(super) fn text_search_sql() -> String {
    // The title the user sees, whitespace collapsed like `title_match` does.
    let shown_title =
        "btrim(regexp_replace(COALESCE(s.title, s.first_message, ''), '\\s+', ' ', 'g'))";
    // Joins `best_scores b` against `thread_summaries s`, so the alias is `s`.
    let thread_cols_prefixed = thread_cols("s");
    let home_visible = home_visible_sql("s");

    // Message text: the long tokens find candidate threads through the trigram
    // index. The short tokens are then checked only on those threads' messages.
    format!(
        "WITH title_matches AS (\
            SELECT thread_id, 1.0::float8 AS match_score FROM thread_summaries WHERE title ILIKE ALL($1::text[])\
        ), long_token_threads AS (\
            SELECT e.thread_id \
            FROM events e CROSS JOIN unnest($6::text[]) AS t(pattern) \
            WHERE {IS_MESSAGE} AND {MESSAGE_TEXT} ILIKE t.pattern \
            GROUP BY e.thread_id \
            HAVING COUNT(DISTINCT t.pattern) = cardinality($6::text[])\
        ), content_matches AS (\
            SELECT c.thread_id, 0.7::float8 AS match_score FROM long_token_threads c \
            WHERE NOT EXISTS (\
                SELECT 1 FROM unnest($7::text[]) AS t(pattern) \
                WHERE NOT EXISTS (\
                    SELECT 1 FROM events e \
                    WHERE e.thread_id = c.thread_id AND {IS_MESSAGE} AND {MESSAGE_TEXT} ILIKE t.pattern\
                )\
            )\
        ), entity_matches AS (\
            /* Drive from memory_entries (the small side) and join into events by \
               primary key. The CASE keeps the cast off non-event sources. */\
            SELECT m.thread_id, 0.7::float8 AS match_score FROM (\
                SELECT e.thread_id, t.pattern \
                FROM memory_entries me \
                JOIN events e ON e.id = CASE WHEN me.source->>'type' = 'event' THEN (me.source->>'id')::uuid END \
                CROSS JOIN unnest($1::text[]) AS t(pattern) \
                WHERE me.source->>'type' = 'event' \
                  AND {IS_MESSAGE} \
                  AND EXISTS (\
                      SELECT 1 FROM jsonb_array_elements_text(me.entities) AS ent \
                      WHERE ent ILIKE t.pattern\
                  )\
            ) m \
            GROUP BY m.thread_id \
            HAVING COUNT(DISTINCT m.pattern) = $3\
        ), draft_matches AS (\
            SELECT thread_id, 0.7::float8 AS match_score FROM thread_summaries \
            WHERE compose_text ILIKE ALL($1::text[])\
        ), scored AS (\
            SELECT thread_id, match_score FROM title_matches \
            UNION ALL \
            SELECT thread_id, match_score FROM content_matches \
            UNION ALL \
            SELECT thread_id, match_score FROM entity_matches \
            UNION ALL \
            SELECT thread_id, match_score FROM draft_matches\
        ), best_scores AS (\
            SELECT thread_id, MAX(match_score) AS score FROM scored GROUP BY thread_id\
        ) \
        SELECT {thread_cols_prefixed}, b.score \
        FROM best_scores b JOIN thread_summaries s ON s.thread_id = b.thread_id \
        WHERE {home_visible} \
        ORDER BY {shown_title} ILIKE $5 DESC, {shown_title} ILIKE $4 DESC, \
                 b.score DESC, s.last_activity DESC LIMIT $2",
    )
}

/// One message event a text search matched.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessageMatch {
    pub event_id: uuid::Uuid,
    pub thread_id: uuid::Uuid,
    pub created: chrono::DateTime<chrono::Utc>,
    pub text: String,
}

impl EventStore {
    /// Search threads by text query (ILIKE on titles, message content and the
    /// unsent draft).
    /// Multi-token queries match per-token: every whitespace-separated token must
    /// appear (case-insensitive) somewhere in the thread, in the title or any
    /// message, but they need not appear together as a phrase. A query of only
    /// short tokens skips message text, which no index can serve. A draft
    /// matches only when it holds every token itself. Title-only
    /// matches score 1.0; content and draft matches score 0.7. The `limit` cut keeps an
    /// exact title first, then a title holding the query as a phrase. Those are
    /// the tiers `thread_search::rank_order` ranks above every other hit.
    pub async fn search_threads_by_text(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        let tokens: Vec<&str> = query.split_whitespace().collect();
        if tokens.is_empty() {
            return Ok(vec![]);
        }
        // Each arm counts DISTINCT patterns against the token count, so a
        // repeated token would make that count unreachable.
        let mut distinct: Vec<&str> = Vec::with_capacity(tokens.len());
        for token in &tokens {
            if !distinct.contains(token) {
                distinct.push(token);
            }
        }
        let patterns: Vec<String> = distinct.iter().map(|t| contains_pattern(t)).collect();
        let (long, short): (Vec<&str>, Vec<&str>) = distinct.iter().partition(|t| is_indexable(t));
        let long_patterns: Vec<String> = long.iter().map(|t| contains_pattern(t)).collect();
        let short_patterns: Vec<String> = short.iter().map(|t| contains_pattern(t)).collect();
        let token_count = patterns.len() as i64;
        let exact_pattern = super::super::escape_like(&tokens.join(" "));
        let phrase_pattern = format!("%{exact_pattern}%");

        #[derive(sqlx::FromRow)]
        struct SearchRow {
            #[sqlx(flatten)]
            row: ThreadRow,
            score: f64,
        }

        let rows = sqlx::query_as::<_, SearchRow>(&text_search_sql())
            .bind(&patterns)
            .bind(limit)
            .bind(token_count)
            .bind(&phrase_pattern)
            .bind(&exact_pattern)
            .bind(&long_patterns)
            .bind(&short_patterns)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|r| {
                Ok(ThreadSearchResult {
                    info: row_to_thread_summary(r.row)?,
                    score: r.score,
                })
            })
            .collect()
    }

    /// The newest message events holding every token of `query`, case
    /// insensitive. The long tokens go through the trigram index; the short
    /// ones are checked on the rows it finds. A query with no token of 3
    /// characters is refused, since no index could serve it.
    pub async fn search_message_events(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<MessageMatch>, Box<dyn std::error::Error + Send + Sync>> {
        let mut distinct: Vec<&str> = Vec::new();
        for token in query.split_whitespace() {
            if !distinct.contains(&token) {
                distinct.push(token);
            }
        }
        let (long, short): (Vec<&str>, Vec<&str>) = distinct.iter().partition(|t| is_indexable(t));
        if long.is_empty() {
            return Err(format!(
                "search needs a word of {MIN_INDEXED_TOKEN_CHARS} or more letters"
            )
            .into());
        }
        let long_patterns: Vec<String> = long.iter().map(|t| contains_pattern(t)).collect();
        let short_patterns: Vec<String> = short.iter().map(|t| contains_pattern(t)).collect();
        // A hidden home thread stays out, as it does from thread search.
        let home_visible = home_visible_sql("h");
        let sql = format!(
            "SELECT e.id AS event_id, e.thread_id, e.created, {MESSAGE_TEXT} AS text \
             FROM events e CROSS JOIN unnest($1::text[]) AS t(pattern) \
             WHERE {IS_MESSAGE} AND {MESSAGE_TEXT} ILIKE t.pattern \
               AND NOT EXISTS (SELECT 1 FROM thread_summaries h \
                               WHERE h.thread_id = e.thread_id AND NOT {home_visible}) \
               AND NOT EXISTS (SELECT 1 FROM unnest($3::text[]) AS s(pattern) \
                               WHERE {MESSAGE_TEXT} NOT ILIKE s.pattern) \
             GROUP BY e.id \
             HAVING COUNT(DISTINCT t.pattern) = cardinality($1::text[]) \
             ORDER BY e.created DESC LIMIT $2"
        );
        Ok(sqlx::query_as::<_, MessageMatch>(&sql)
            .bind(&long_patterns)
            .bind(limit)
            .bind(&short_patterns)
            .fetch_all(&self.pool)
            .await?)
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

        // No SQL LIMIT: the cut belongs after the score sort below. An
        // unordered `LIMIT` here returns an arbitrary slice of the candidates,
        // so the best-scoring thread is dropped whenever Postgres did not
        // happen to return it. `thread_uuids` is already bounded by the
        // caller's SEMANTIC_CANDIDATE_LIMIT.
        let sql = format!(
            "SELECT {} FROM thread_summaries t WHERE t.thread_id = ANY($1::uuid[]) AND {}",
            THREAD_COLS.as_str(),
            home_visible_sql("t"),
        );
        let rows = sqlx::query_as::<_, ThreadRow>(&sql)
            .bind(&thread_uuids)
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
        results.truncate(limit.max(0) as usize);
        Ok(results)
    }
}
