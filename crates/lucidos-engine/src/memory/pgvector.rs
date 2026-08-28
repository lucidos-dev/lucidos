use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Width of `memory_entries.embedding` on a workspace created by THIS build.
///
/// Only used in the `CREATE TABLE IF NOT EXISTS` below, which means it does not
/// govern an existing workspace: that column keeps whatever width it was created
/// with, forever, and changing this constant would not migrate it. So nothing
/// may assume the loaded model's width equals this. Ask
/// [`PgVectorIndex::embedding_column_dimensions`] what the table actually has.
const VECTOR_DIM: i32 = 384;

/// Pull `N` out of a rendered pgvector column type, e.g. `vector(384)`.
/// `None` for anything else, including a width-less `vector` (pgvector allows
/// an unconstrained column, which imposes no width to disagree with).
fn parse_vector_dimensions(rendered: &str) -> Option<usize> {
    let inner = rendered.trim().strip_prefix("vector")?.trim();
    inner
        .strip_prefix('(')?
        .strip_suffix(')')?
        .trim()
        .parse()
        .ok()
}

/// Format a vector as a pgvector literal — `[1.0,2.0,...]`.
/// pgvector accepts this string form via the `::vector` cast in queries.
fn to_pgvector_literal(v: &[f32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemorySource {
    #[serde(rename = "artifact")]
    Artifact { path: String, commit: String },
    #[serde(rename = "event")]
    Event { id: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Uuid,
    pub source: MemorySource,
    pub topic: String,
    pub summary: String,
    pub importance: f32,
    pub entities: Vec<String>,
    /// Timestamp from the source event or artifact (when the content was originally created)
    pub src_created_at: DateTime<Utc>,
    /// Timestamp when this memory entry row was inserted/updated
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct SearchResult {
    pub entries: Vec<MemoryEntry>,
}

#[derive(Debug, Clone)]
pub struct SimilarEntry {
    pub id: Uuid,
    pub topic: String,
    pub importance: f32,
    pub entities: Vec<String>,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportanceDistribution {
    pub low: i64,
    pub medium: i64,
    pub high: i64,
    pub critical: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicCount {
    pub topic: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total: i64,
    pub event_count: i64,
    pub artifact_count: i64,
    pub importance_distribution: ImportanceDistribution,
    pub top_topics: Vec<TopicCount>,
}

/// PostgreSQL + pgvector backed memory index
/// Stores embeddings and summaries directly in PostgreSQL
#[derive(Clone)]
pub struct PgVectorIndex {
    pool: PgPool,
}

impl PgVectorIndex {
    /// Create a new pgvector index, initializing the schema.
    pub async fn new(pool: PgPool) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let index = Self { pool };
        index.init_schema().await?;
        Ok(index)
    }

    /// The width `memory_entries.embedding` ACTUALLY has, read from the live
    /// table rather than assumed from [`VECTOR_DIM`].
    ///
    /// This is what a loaded model has to agree with. The table is created
    /// `IF NOT EXISTS`, so a workspace provisioned by an older build keeps its
    /// original width and no constant in this binary describes it. Without the
    /// check, a model of a different width installs fine and then every insert
    /// and every re-embed UPDATE fails on a dimension mismatch, with
    /// `reembed_stale` aborting its sweep (correctly, to avoid an infinite
    /// loop) and telling nobody: memory quietly stops taking writes.
    ///
    /// Parsed out of `format_type`'s `vector(N)` rather than read from
    /// `atttypmod` directly, so it does not depend on how pgvector chooses to
    /// encode the modifier. `None` when the table or column is absent (a
    /// brand-new workspace mid-provision), which is not a mismatch.
    pub async fn embedding_column_dimensions(
        &self,
    ) -> Result<Option<usize>, Box<dyn std::error::Error + Send + Sync>> {
        let rendered: Option<String> = sqlx::query_scalar(
            r#"
            SELECT format_type(a.atttypid, a.atttypmod)
            FROM pg_attribute a
            WHERE a.attrelid = to_regclass('memory_entries')
              AND a.attname = 'embedding'
              AND NOT a.attisdropped
            "#,
        )
        .fetch_optional(&self.pool)
        .await?
        .flatten();

        Ok(rendered.and_then(|t| parse_vector_dimensions(&t)))
    }

    /// Initialize the pgvector extension and create tables.
    async fn init_schema(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Enable pgvector extension
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&self.pool)
            .await?;

        // Create memory entries table
        // `extractor_version` is also added by a migration (idempotent ALTER) for
        // upgrades; including it here covers fresh installs where init_schema
        // creates the table from scratch.
        sqlx::query(&format!(
            r#"
            CREATE TABLE IF NOT EXISTS memory_entries (
                id UUID PRIMARY KEY,
                source JSONB NOT NULL,
                topic TEXT NOT NULL,
                summary TEXT NOT NULL,
                importance REAL NOT NULL,
                entities JSONB NOT NULL DEFAULT '[]',
                embedding vector({}) NOT NULL,
                embedding_model TEXT NOT NULL,
                src_created_at TIMESTAMPTZ NOT NULL,
                created_at TIMESTAMPTZ NOT NULL,
                extractor_version INTEGER NOT NULL DEFAULT 0
            )
            "#,
            VECTOR_DIM
        ))
        .execute(&self.pool)
        .await?;

        // HNSW index with tuned build parameters for better recall.
        // m=24 (default 16): more connections per node = better recall, slightly more storage.
        // ef_construction=200 (default 64): more candidates during build = higher quality graph.
        // Drop the old default-params index if it exists (one-time migration to tuned params).
        sqlx::query("DROP INDEX IF EXISTS memory_entries_embedding_idx")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_embedding_hnsw_idx
            ON memory_entries
            USING hnsw (embedding vector_cosine_ops)
            WITH (m = 24, ef_construction = 200)
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Create B-tree index on topic for filtering
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_topic_idx
            ON memory_entries (topic)
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_embedding_model_idx
            ON memory_entries (embedding_model)
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Create B-tree index on importance for filtering
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_importance_idx
            ON memory_entries (importance)
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Index on src_created_at for chronological queries (source time)
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_src_created_at_idx
            ON memory_entries (src_created_at DESC)
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Partial expression index on source->>'id' for event-source rows.
        // Used by search_threads_by_text's entity_matches CTE to look up the
        // event row for each memory entry. Partial (WHERE source->>'type' =
        // 'event') keeps the index small since artifact-source rows are never
        // joined this way.
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_source_event_id_idx
            ON memory_entries ((source->>'id'))
            WHERE source->>'type' = 'event'
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Trigram index on summary for fast ILIKE keyword search
        sqlx::query("CREATE EXTENSION IF NOT EXISTS pg_trgm")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS memory_entries_summary_trgm_idx
            ON memory_entries USING gin (summary gin_trgm_ops)
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // One arg per indexed column (id, source, topic, summary, importance, entities,
    // embedding, embedding_model, src_created_at, extractor_version) — a struct
    // wrapper would mirror the row shape with no readability gain.
    #[allow(clippy::too_many_arguments)]
    pub async fn index_entry(
        &self,
        id: Uuid,
        source: &MemorySource,
        topic: &str,
        summary: &str,
        importance: f32,
        entities: &[String],
        embedding: &[f32],
        embedding_model: &str,
        src_created_at: DateTime<Utc>,
        extractor_version: i32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let source_json = serde_json::to_value(source)?;
        let entities_json = serde_json::to_value(entities)?;
        let created_at = Utc::now();
        let embedding_str = to_pgvector_literal(embedding);

        // Upsert: insert or update on conflict
        sqlx::query(
            r#"
            INSERT INTO memory_entries (id, source, topic, summary, importance, entities, embedding, embedding_model, src_created_at, created_at, extractor_version)
            VALUES ($1, $2, $3, $4, $5, $6, $7::vector, $8, $9, $10, $11)
            ON CONFLICT (id) DO UPDATE SET
                source = EXCLUDED.source,
                topic = EXCLUDED.topic,
                summary = EXCLUDED.summary,
                importance = EXCLUDED.importance,
                entities = EXCLUDED.entities,
                embedding = EXCLUDED.embedding,
                embedding_model = EXCLUDED.embedding_model,
                src_created_at = EXCLUDED.src_created_at,
                created_at = EXCLUDED.created_at,
                extractor_version = EXCLUDED.extractor_version
            "#,
        )
        .bind(id)
        .bind(&source_json)
        .bind(topic)
        .bind(summary)
        .bind(importance)
        .bind(&entities_json)
        .bind(&embedding_str)
        .bind(embedding_model)
        .bind(src_created_at)
        .bind(created_at)
        .bind(extractor_version)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn update_embedding(
        &self,
        id: Uuid,
        embedding: &[f32],
        embedding_model: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let embedding_str = to_pgvector_literal(embedding);
        sqlx::query(
            "UPDATE memory_entries SET embedding = $1::vector, embedding_model = $2 WHERE id = $3",
        )
        .bind(&embedding_str)
        .bind(embedding_model)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn count_stale_embeddings(
        &self,
        current_model: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let count =
            sqlx::query_scalar("SELECT COUNT(*) FROM memory_entries WHERE embedding_model <> $1")
                .bind(current_model)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    pub async fn fetch_stale_embedding_batch(
        &self,
        current_model: &str,
        limit: i64,
    ) -> Result<Vec<(Uuid, String)>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query_as(
            "SELECT id, summary FROM memory_entries WHERE embedding_model <> $1 LIMIT $2",
        )
        .bind(current_model)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Search by embedding and return cosine similarity scores alongside entries.
    /// Similarity is 1.0 - cosine_distance (1.0 = identical, 0.0 = orthogonal).
    pub async fn search_with_scores(
        &self,
        query_embedding: &[f32],
        min_importance: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f64)>, Box<dyn std::error::Error + Send + Sync>> {
        let embedding_str = to_pgvector_literal(query_embedding);

        let rows = sqlx::query(
            r#"
            SELECT id, source, topic, summary, importance, entities, src_created_at, created_at,
                   1.0 - (embedding <=> $2::vector) AS similarity
            FROM memory_entries
            WHERE importance >= $1
            ORDER BY embedding <=> $2::vector
            LIMIT $3
            "#,
        )
        .bind(min_importance)
        .bind(&embedding_str)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        use sqlx::Row;
        let results = rows
            .iter()
            .map(|row| {
                let entry = row_to_memory_entry(row)?;
                let similarity: f64 = row.try_get("similarity")?;
                Ok((entry, similarity))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()?;

        Ok(results)
    }

    /// Search by keyword — matches against entities JSONB array and summary text (ILIKE)
    pub async fn search_by_keyword(
        &self,
        keyword: &str,
        min_importance: f32,
        limit: usize,
    ) -> Result<SearchResult, Box<dyn std::error::Error + Send + Sync>> {
        // The keyword is model-supplied, so its metacharacters must match
        // literally: a bare `%` otherwise matches every row above the
        // importance floor.
        let like_pattern = format!("%{}%", crate::core::store::escape_like(keyword));

        let rows = sqlx::query(
            r#"
            SELECT id, source, topic, summary, importance, entities, src_created_at, created_at
            FROM memory_entries
            WHERE importance >= $1
              AND (
                  summary ILIKE $2
                  OR EXISTS (
                      SELECT 1 FROM jsonb_array_elements_text(entities) AS e
                      WHERE e ILIKE $2
                  )
              )
            ORDER BY importance DESC, src_created_at DESC
            LIMIT $3
            "#,
        )
        .bind(min_importance)
        .bind(&like_pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .iter()
            .map(|row| row_to_memory_entry(row))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SearchResult { entries })
    }

    /// Get entry count
    pub async fn len(&self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_entries")
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0 as usize)
    }

    /// Cheap emptiness check — `SELECT EXISTS` short-circuits on the first
    /// row, so this stays O(1) on a populated table rather than scanning to
    /// compute `COUNT(*)` like [`Self::len`].
    pub async fn is_empty(&self) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let exists: (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM memory_entries)")
            .fetch_one(&self.pool)
            .await?;
        Ok(!exists.0)
    }

    /// Fetch a single entry by its primary key, or `None` if it doesn't exist.
    /// The `correct_memory_by_id` tool uses this to confirm the target exists
    /// (and to echo what was removed) before deleting it.
    pub async fn get_by_id(
        &self,
        id: Uuid,
    ) -> Result<Option<MemoryEntry>, Box<dyn std::error::Error + Send + Sync>> {
        let row = sqlx::query(
            r#"
            SELECT id, source, topic, summary, importance, entities, src_created_at, created_at
            FROM memory_entries
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_memory_entry(&row)?)),
            None => Ok(None),
        }
    }

    /// Delete an entry
    pub async fn delete(&self, id: Uuid) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM memory_entries WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Find entries similar to the given embedding above a similarity threshold.
    /// Uses the existing HNSW index for fast approximate nearest-neighbor search.
    pub async fn find_similar(
        &self,
        embedding: &[f32],
        min_similarity: f32,
        limit: usize,
    ) -> Result<Vec<SimilarEntry>, Box<dyn std::error::Error + Send + Sync>> {
        use sqlx::Row;

        let embedding_str = to_pgvector_literal(embedding);

        let rows = sqlx::query(
            r#"
            SELECT id, topic, importance, entities, 1 - (embedding <=> $1::vector) AS similarity
            FROM memory_entries
            WHERE 1 - (embedding <=> $1::vector) > $2
            ORDER BY similarity DESC
            LIMIT $3
            "#,
        )
        .bind(&embedding_str)
        .bind(min_similarity)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .iter()
            .map(|row| {
                let entities_json: serde_json::Value = row.try_get("entities")?;
                let entities: Vec<String> = serde_json::from_value(entities_json)?;
                Ok(SimilarEntry {
                    id: row.try_get("id")?,
                    topic: row.try_get("topic")?,
                    importance: row.try_get("importance")?,
                    entities,
                    similarity: row.try_get("similarity")?,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()?;

        Ok(entries)
    }

    /// Delete multiple entries by ID in a single query (for batch supersession).
    pub async fn delete_many(
        &self,
        ids: &[Uuid],
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query("DELETE FROM memory_entries WHERE id = ANY($1)")
            .bind(ids)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete all memory entries (for full rebuild from scratch)
    pub async fn clear(&self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM memory_entries")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    /// Delete entries whose extractor_version is below `min_version`.
    /// Returns the number of rows deleted. After this, `sources_indexed`
    /// will report the affected sources as un-indexed, so the normal
    /// incremental rebuild loop re-extracts them with the current extractor.
    pub async fn delete_below_extractor_version(
        &self,
        min_version: i32,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let result = sqlx::query("DELETE FROM memory_entries WHERE extractor_version < $1")
            .bind(min_version)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Batch-check which sources are already indexed in memory_entries.
    /// Returns the set of source JSON values that already have entries.
    /// Values are re-serialized via serde_json to normalize key order for comparison.
    pub async fn sources_indexed(
        &self,
        sources: &[serde_json::Value],
    ) -> Result<std::collections::HashSet<String>, Box<dyn std::error::Error + Send + Sync>> {
        if sources.is_empty() {
            return Ok(std::collections::HashSet::new());
        }

        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT DISTINCT source FROM memory_entries WHERE source = ANY($1)")
                .bind(sources)
                .fetch_all(&self.pool)
                .await?;

        // Re-serialize through serde_json to get consistent key ordering.
        // Propagate (rather than swallow to "") a serialize failure: it can't
        // happen for well-formed JSONB, but surfacing it beats silently
        // inserting an empty source into the indexed set.
        rows.into_iter()
            .map(|(v,)| serde_json::to_string(&v).map_err(Into::into))
            .collect()
    }

    /// Paginated list of memory entries with optional source type filter.
    /// `source_type` can be "event" or "artifact" to filter, or None for all.
    /// `sort` can be "importance" to sort by importance desc, otherwise sorts by created_at desc.
    pub async fn paginated_entries(
        &self,
        limit: i64,
        offset: i64,
        source_type: Option<&str>,
        sort: Option<&str>,
        importance_levels: Option<&[&str]>,
    ) -> Result<(Vec<MemoryEntry>, i64), Box<dyn std::error::Error + Send + Sync>> {
        let mut conditions: Vec<String> = Vec::new();

        if let Some(st) = source_type {
            let validated = match st {
                "event" => "event",
                "artifact" => "artifact",
                other => {
                    return Err(format!(
                        "invalid source_type '{}': expected 'event' or 'artifact'",
                        other
                    )
                    .into());
                }
            };
            conditions.push(format!("source->>'type' = '{}'", validated));
        }

        if let Some(levels) = importance_levels {
            if !levels.is_empty() {
                let range_conditions: Vec<&str> = levels
                    .iter()
                    .filter_map(|l| match *l {
                        "low" => Some("importance < 0.3"),
                        "medium" => Some("(importance >= 0.3 AND importance < 0.6)"),
                        "high" => Some("(importance >= 0.6 AND importance < 0.8)"),
                        "critical" => Some("importance >= 0.8"),
                        _ => None,
                    })
                    .collect();
                if !range_conditions.is_empty() {
                    conditions.push(format!("({})", range_conditions.join(" OR ")));
                }
            }
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let order_clause = if sort == Some("importance") {
            "ORDER BY importance DESC, created_at DESC"
        } else {
            "ORDER BY created_at DESC"
        };

        let count_query = format!("SELECT COUNT(*) FROM memory_entries {}", where_clause);
        let total: (i64,) = sqlx::query_as(&count_query).fetch_one(&self.pool).await?;

        let entries_query = format!(
            "SELECT id, source, topic, summary, importance, entities, src_created_at, created_at FROM memory_entries {} {} LIMIT $1 OFFSET $2",
            where_clause, order_clause
        );
        let rows = sqlx::query(&entries_query)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

        let entries = rows
            .iter()
            .map(|row| row_to_memory_entry(row))
            .collect::<Result<Vec<_>, _>>()?;

        Ok((entries, total.0))
    }

    /// Get all memory entries extracted from a given source (by JSONB match).
    pub async fn entries_for_source(
        &self,
        source: &serde_json::Value,
    ) -> Result<Vec<MemoryEntry>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = sqlx::query(
            r#"
            SELECT id, source, topic, summary, importance, entities, src_created_at, created_at
            FROM memory_entries
            WHERE source @> $1
            ORDER BY importance DESC
            "#,
        )
        .bind(source)
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .iter()
            .map(|row| row_to_memory_entry(row))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Aggregate stats about memory entries.
    pub async fn stats(&self) -> Result<MemoryStats, Box<dyn std::error::Error + Send + Sync>> {
        use sqlx::Row;

        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM memory_entries")
            .fetch_one(&self.pool)
            .await?;

        let event_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM memory_entries WHERE source->>'type' = 'event'")
                .fetch_one(&self.pool)
                .await?;

        let artifact_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM memory_entries WHERE source->>'type' = 'artifact'",
        )
        .fetch_one(&self.pool)
        .await?;

        // Importance distribution buckets
        let importance_rows = sqlx::query(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE importance < 0.3) AS low,
                COUNT(*) FILTER (WHERE importance >= 0.3 AND importance < 0.6) AS medium,
                COUNT(*) FILTER (WHERE importance >= 0.6 AND importance < 0.8) AS high,
                COUNT(*) FILTER (WHERE importance >= 0.8) AS critical
            FROM memory_entries
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let importance_distribution = ImportanceDistribution {
            low: importance_rows.try_get::<i64, _>("low")?,
            medium: importance_rows.try_get::<i64, _>("medium")?,
            high: importance_rows.try_get::<i64, _>("high")?,
            critical: importance_rows.try_get::<i64, _>("critical")?,
        };

        // Top 10 topics by count
        let topic_rows = sqlx::query(
            "SELECT topic, COUNT(*) as count FROM memory_entries GROUP BY topic ORDER BY count DESC LIMIT 10"
        )
        .fetch_all(&self.pool)
        .await?;

        let top_topics = topic_rows
            .iter()
            .map(|row| {
                Ok(TopicCount {
                    topic: row.try_get("topic")?,
                    count: row.try_get::<i64, _>("count")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;

        Ok(MemoryStats {
            total: total.0,
            event_count: event_count.0,
            artifact_count: artifact_count.0,
            importance_distribution,
            top_topics,
        })
    }
}

/// Extract a MemoryEntry from a sqlx Row using manual column access
fn row_to_memory_entry(
    row: &sqlx::postgres::PgRow,
) -> Result<MemoryEntry, Box<dyn std::error::Error + Send + Sync>> {
    use sqlx::Row;

    let id: Uuid = row.try_get("id")?;
    let source_json: serde_json::Value = row.try_get("source")?;
    let topic: String = row.try_get("topic")?;
    let summary: String = row.try_get("summary")?;
    let importance: f32 = row.try_get("importance")?;
    let entities_json: serde_json::Value = row.try_get("entities")?;
    let src_created_at: DateTime<Utc> = row.try_get("src_created_at")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;

    let source: MemorySource = serde_json::from_value(source_json)?;
    let entities: Vec<String> = serde_json::from_value(entities_json)?;

    Ok(MemoryEntry {
        id,
        source,
        topic,
        summary,
        importance,
        entities,
        src_created_at,
        created_at,
    })
}

#[cfg(test)]
mod extractor_version_tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// `index_entry` must persist whatever extractor_version it was given —
    /// otherwise `delete_below_extractor_version` is looking at a column that
    /// always reads 0.
    #[tokio::test]
    async fn index_entry_persists_extractor_version() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();

        let id = Uuid::new_v4();
        index
            .index_entry(
                id,
                &MemorySource::Event { id: Uuid::new_v4() },
                "topic",
                "summary",
                0.5,
                &[],
                &vec![0.1f32; 384],
                "model",
                Utc::now(),
                7,
            )
            .await
            .unwrap();

        let stored: i32 =
            sqlx::query_scalar("SELECT extractor_version FROM memory_entries WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, 7);

        teardown_test_db(&db_name).await;
    }

    /// `delete_below_extractor_version(N)` must drop rows with version < N
    /// and leave rows with version >= N alone — that is the whole contract
    /// the rebuild relies on for re_extract_stale.
    #[tokio::test]
    async fn delete_below_extractor_version_only_touches_old_rows() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();

        let stale_id = Uuid::new_v4();
        let current_id = Uuid::new_v4();
        let future_id = Uuid::new_v4();

        for (id, version) in [(stale_id, 0), (current_id, 1), (future_id, 2)] {
            index
                .index_entry(
                    id,
                    &MemorySource::Event { id: Uuid::new_v4() },
                    "topic",
                    "summary",
                    0.5,
                    &[],
                    &vec![0.1f32; 384],
                    "model",
                    Utc::now(),
                    version,
                )
                .await
                .unwrap();
        }

        let deleted = index.delete_below_extractor_version(1).await.unwrap();
        assert_eq!(deleted, 1, "only the stale row should be deleted");

        let remaining: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM memory_entries ORDER BY extractor_version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, vec![current_id, future_id]);

        // Re-running the same delete is a no-op now that no rows are below 1.
        let deleted_again = index.delete_below_extractor_version(1).await.unwrap();
        assert_eq!(deleted_again, 0);

        teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod get_by_id_tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// `get_by_id` round-trips an inserted entry and returns `None` for an
    /// unknown id — the lookup the `correct_memory_by_id` tool relies on to
    /// distinguish "deleted that one" from "no such id" (a safe no-op).
    #[tokio::test]
    async fn get_by_id_round_trips_and_misses_cleanly() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();

        let id = Uuid::new_v4();
        index
            .index_entry(
                id,
                &MemorySource::Event { id: Uuid::new_v4() },
                "Config",
                "Config dir is at gws-personal",
                0.8,
                &["gws-personal".to_string()],
                &vec![0.1f32; 384],
                "model",
                Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();

        let found = index
            .get_by_id(id)
            .await
            .unwrap()
            .expect("inserted entry should be found by id");
        assert_eq!(found.id, id);
        assert_eq!(found.topic, "Config");
        assert_eq!(found.summary, "Config dir is at gws-personal");
        assert_eq!(found.entities, vec!["gws-personal".to_string()]);

        let missing = index.get_by_id(Uuid::new_v4()).await.unwrap();
        assert!(
            missing.is_none(),
            "unknown id must return None, not an entry"
        );

        teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod embedding_dimension_tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// The parse has to survive whatever `format_type` renders, and must not
    /// invent a width for an unconstrained column (which imposes none).
    #[test]
    fn parses_only_a_width_constrained_vector_type() {
        assert_eq!(parse_vector_dimensions("vector(384)"), Some(384));
        assert_eq!(parse_vector_dimensions("vector(1024)"), Some(1024));
        assert_eq!(parse_vector_dimensions(" vector(768) "), Some(768));
        // pgvector allows a width-less column; nothing to disagree with.
        assert_eq!(parse_vector_dimensions("vector"), None);
        // Never guess from a different type.
        assert_eq!(parse_vector_dimensions("halfvec(384)"), None);
        assert_eq!(parse_vector_dimensions("text"), None);
        assert_eq!(parse_vector_dimensions("vector(oops)"), None);
    }

    /// Read the width from the LIVE table, not from `VECTOR_DIM`: an existing
    /// workspace keeps whatever width its column was created with, and the
    /// dimension guard has to judge a model against that.
    #[tokio::test]
    async fn reports_the_live_columns_width() {
        let (pool, db_name) = setup_test_db().await;

        // Before the table exists there is no column, which is not a mismatch.
        let bare = PgVectorIndex { pool: pool.clone() };
        assert_eq!(bare.embedding_column_dimensions().await.unwrap(), None);

        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        assert_eq!(
            index.embedding_column_dimensions().await.unwrap(),
            Some(VECTOR_DIM as usize),
            "a freshly created table reports the width this build creates"
        );

        // A workspace whose column was created at a different width must
        // report THAT, which is the whole point of asking the table.
        sqlx::query("ALTER TABLE memory_entries ALTER COLUMN embedding TYPE vector(7)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(index.embedding_column_dimensions().await.unwrap(), Some(7));

        teardown_test_db(&db_name).await;
    }
}
