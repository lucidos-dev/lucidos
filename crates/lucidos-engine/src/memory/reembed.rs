//! Background task that re-embeds memory_entries rows whose embedding_model
//! does not match the currently configured model. Runs at startup; idempotent.

use crate::memory::provider::EmbeddingProvider;
use crate::memory::PgVectorIndex;
use std::sync::Arc;

/// Rows fetched and re-embedded per loop iteration. 32 is around the sweet spot
/// for E5-small batch inference on CPU (plateau begins around 32-64).
const BATCH_SIZE: i64 = 32;

/// Each row UPDATE is auto-committed independently, so a mid-task crash leaves
/// a consistent partial state the next run picks up (stale rows keep the old id).
pub async fn reembed_stale(
    index: &PgVectorIndex,
    embedder: Arc<dyn EmbeddingProvider>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let target_model = embedder.model_id().to_string();

    let stale_count = index.count_stale_embeddings(&target_model).await?;
    if stale_count == 0 {
        log!(@Memory, "No stale embeddings to re-process");
        return Ok(0);
    }

    log!(
        @Memory,
        "Re-embedding {} stale rows for model '{}'",
        stale_count,
        target_model
    );

    let mut updated = 0usize;
    let mut failed = 0usize;
    loop {
        let batch = index
            .fetch_stale_embedding_batch(&target_model, BATCH_SIZE)
            .await?;
        if batch.is_empty() {
            break;
        }

        let texts: Vec<&str> = batch.iter().map(|(_, s)| s.as_str()).collect();
        let embeddings = embedder.embed_batch(&texts).await?;

        let mut batch_updated = 0usize;
        for ((id, _), embedding) in batch.iter().zip(embeddings.iter()) {
            match index.update_embedding(*id, embedding, &target_model).await {
                Ok(_) => {
                    updated += 1;
                    batch_updated += 1;
                }
                Err(e) => {
                    failed += 1;
                    log!(@Memory, "Re-embed UPDATE failed for row {}: {}", id, e);
                }
            }
        }

        log!(@Memory, "Re-embed progress: {} / {}", updated, stale_count);

        // Guard against an infinite loop when every row in a batch fails
        // (e.g. dimension mismatch). The same rows would otherwise be re-fetched forever.
        if batch_updated == 0 {
            log!(@Memory, "Re-embed aborted: no rows in batch could be updated");
            break;
        }

        // Pace the loop so other DB consumers aren't starved during startup.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    if failed > 0 {
        log!(
            @Memory,
            "Re-embed complete: {} rows updated, {} rows failed",
            updated,
            failed
        );
    } else {
        log!(@Memory, "Re-embed complete: {} rows updated", updated);
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemorySource;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use chrono::Utc;
    use uuid::Uuid;

    /// Test embedder that returns a fixed vector and a configurable model id.
    struct StubEmbedder {
        id: String,
        vec: Vec<f32>,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(
            &self,
            _text: &str,
        ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.vec.clone())
        }
        async fn embed_batch(
            &self,
            texts: &[&str],
        ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
            Ok(texts.iter().map(|_| self.vec.clone()).collect())
        }
        fn dimensions(&self) -> usize {
            self.vec.len()
        }
        fn model_id(&self) -> &str {
            &self.id
        }
    }

    #[tokio::test]
    async fn test_reembed_updates_only_stale_rows() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();

        let old_vec = vec![0.1f32; 384];
        let new_vec = vec![0.9f32; 384];
        let old_id = Uuid::new_v4();
        let new_id = Uuid::new_v4();

        index
            .index_entry(
                old_id,
                &MemorySource::Event { id: Uuid::new_v4() },
                "topic",
                "old summary",
                0.5,
                &[],
                &old_vec,
                "old-model",
                Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();

        index
            .index_entry(
                new_id,
                &MemorySource::Event { id: Uuid::new_v4() },
                "topic",
                "new summary",
                0.5,
                &[],
                &new_vec,
                "new-model",
                Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();

        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder {
            id: "new-model".to_string(),
            vec: new_vec.clone(),
        });
        let updated = reembed_stale(&index, embedder).await.unwrap();
        assert_eq!(updated, 1, "only the stale row should be updated");

        let old_row_model: String =
            sqlx::query_scalar("SELECT embedding_model FROM memory_entries WHERE id = $1")
                .bind(old_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(old_row_model, "new-model");

        let new_row_model: String =
            sqlx::query_scalar("SELECT embedding_model FROM memory_entries WHERE id = $1")
                .bind(new_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(new_row_model, "new-model");

        let stale = index.count_stale_embeddings("new-model").await.unwrap();
        assert_eq!(stale, 0);

        teardown_test_db(&db_name).await;
    }

    /// If every UPDATE in a batch fails (e.g. dimension mismatch), reembed_stale
    /// must terminate rather than re-fetch the same rows forever.
    #[tokio::test]
    async fn test_reembed_terminates_when_all_updates_fail() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();

        index
            .index_entry(
                Uuid::new_v4(),
                &MemorySource::Event { id: Uuid::new_v4() },
                "topic",
                "summary",
                0.5,
                &[],
                &vec![0.1f32; 384],
                "old-model",
                Utc::now(),
                crate::memory::EXTRACTOR_VERSION,
            )
            .await
            .unwrap();

        // New embedder returns 7-dim vectors → every UPDATE fails (column is vector(384)).
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder {
            id: "new-model".to_string(),
            vec: vec![0.0; 7],
        });

        // If the loop doesn't terminate this hangs the test; tokio::time::timeout
        // bounds it so a regression is a clear failure rather than an infinite hang.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reembed_stale(&index, embedder),
        )
        .await
        .expect("reembed_stale must terminate when all updates fail");
        assert_eq!(result.unwrap(), 0, "no rows should have been updated");

        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn test_reembed_noop_when_all_current() {
        let (pool, db_name) = setup_test_db().await;
        let index = PgVectorIndex::new(pool.clone()).await.unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder {
            id: "model".to_string(),
            vec: vec![0.0; 384],
        });
        let updated = reembed_stale(&index, embedder).await.unwrap();
        assert_eq!(updated, 0);
        teardown_test_db(&db_name).await;
    }
}
