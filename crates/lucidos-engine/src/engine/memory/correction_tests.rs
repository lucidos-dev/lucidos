use super::*;
use crate::core::EventStore;
use crate::test_support::{setup_test_db, teardown_test_db, Fixed384Embedder};
use async_trait::async_trait;
use chrono::{Duration, Utc};

/// Each word lights one of 384 dimensions, so texts sharing words are close
/// and texts sharing few are far apart. Enough to tell a fact from its
/// neighbour, which the fixed embedder cannot.
struct BagOfWordsEmbedder;

impl BagOfWordsEmbedder {
    fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        for word in text.to_lowercase().split_whitespace() {
            let slot = word
                .bytes()
                .fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
                % 384;
            v[slot] += 1.0;
        }
        v
    }
}

#[async_trait]
impl EmbeddingProvider for BagOfWordsEmbedder {
    async fn embed(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self::vector(text))
    }
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(texts.iter().map(|t| Self::vector(t)).collect())
    }
    fn dimensions(&self) -> usize {
        384
    }
    fn model_id(&self) -> &str {
        "test-bag-of-words"
    }
}

async fn seed(
    index: &PgVectorIndex,
    embedder: &dyn EmbeddingProvider,
    summary: &str,
    entities: &[String],
    source: &MemorySource,
    src_created_at: DateTime<Utc>,
) -> Uuid {
    let id = Uuid::new_v4();
    index
        .index_entry(
            id,
            source,
            "General",
            summary,
            0.5,
            entities,
            &embedder.embed(summary).await.unwrap(),
            embedder.model_id(),
            src_created_at,
            crate::memory::EXTRACTOR_VERSION,
        )
        .await
        .unwrap();
    id
}

/// Record one correction removing `summary` from `source`, and return the
/// stored row a rebuild would replay.
async fn recorded(
    pool: &sqlx::PgPool,
    summary: &str,
    entities: &[String],
    source: &MemorySource,
) -> EventRow {
    let (bus, _rx) = EventBus::new(pool.clone());
    let removed = vec![CorrectedMemory {
        summary: summary.to_string(),
        entities: entities.to_vec(),
        source: source.clone(),
    }];
    record_correction(&bus, summary, summary, removed, None)
        .await
        .unwrap();
    let mut rows = EventStore::new(pool.clone())
        .events_of_type_chronological(MEMORY_CORRECTED)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

/// One correction, any number of rebuilds, one row. The id is derived from
/// the event, so Phase 3 upserts rather than inserting another copy.
#[tokio::test]
async fn replaying_a_correction_twice_leaves_one_entry() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let event_id = Uuid::new_v4();

    for _ in 0..2 {
        index_correction(
            &index,
            &Fixed384Embedder,
            event_id,
            "The dog is called Rex.",
            Utc::now(),
        )
        .await
        .unwrap();
    }

    assert_eq!(
        index.len().await.unwrap(),
        1,
        "a second rebuild must upsert the correction, not duplicate it"
    );

    // And the row points back at the MemoryCorrected event, so
    // `sources_indexed` sees it and "View source" resolves.
    let source_json = serde_json::to_value(MemorySource::Event { id: event_id }).unwrap();
    assert_eq!(
        index.entries_for_source(&source_json).await.unwrap().len(),
        1,
        "the source must be the correction event, not a random uuid"
    );

    teardown_test_db(&db_name).await;
}

/// A correction removes what was believed before it. The same fact stated
/// again afterwards is the user's newer word, and a replay must keep it.
#[tokio::test]
async fn a_replay_keeps_a_fact_restated_after_the_correction() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let wrong = "User works at Acme Corp";
    let source = MemorySource::Event { id: Uuid::new_v4() };
    let event = recorded(&pool, wrong, &[], &source).await;

    let embedder = &Fixed384Embedder;
    let before = seed(
        &index,
        embedder,
        wrong,
        &[],
        &source,
        event.created - Duration::hours(1),
    )
    .await;
    let after = seed(
        &index,
        embedder,
        wrong,
        &[],
        &source,
        event.created + Duration::hours(1),
    )
    .await;
    replay_corrections(&index, embedder, &[event]).await;

    assert!(index.get_by_id(before).await.unwrap().is_none());
    assert!(index.get_by_id(after).await.unwrap().is_some());

    teardown_test_db(&db_name).await;
}

/// The replay has no LLM to confirm a match, so it must only catch the removed
/// fact again. A neighbouring fact from the same message, and a close one from
/// another conversation, were never part of the correction. Both clear the
/// keyword-and-similarity bar that a correction's first search uses.
#[tokio::test]
async fn a_replay_deletes_only_the_removed_fact() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let wrong = "User works at Acme Corp";
    let source = MemorySource::Event { id: Uuid::new_v4() };
    let event = recorded(&pool, wrong, &[], &source).await;

    let embedder = &BagOfWordsEmbedder;
    let older = event.created - Duration::hours(1);
    let re_extracted = seed(&index, embedder, wrong, &[], &source, older).await;
    let neighbour = seed(
        &index,
        embedder,
        "User worked at Acme Corp before",
        &[],
        &source,
        older,
    )
    .await;
    let elsewhere = MemorySource::Event { id: Uuid::new_v4() };
    let other_thread = seed(
        &index,
        embedder,
        "User works at Acme Corp as a contractor",
        &[],
        &elsewhere,
        older,
    )
    .await;
    replay_corrections(&index, embedder, &[event]).await;

    assert!(index.get_by_id(re_extracted).await.unwrap().is_none());
    assert!(index.get_by_id(neighbour).await.unwrap().is_some());
    assert!(index.get_by_id(other_thread).await.unwrap().is_some());

    teardown_test_db(&db_name).await;
}

/// Live extraction skips a fact it already holds, so an assistant echoing the
/// user never gets its own entry. A rebuild extracts both concurrently and can
/// write both, and the copy from the echo is the same wrong fact.
#[tokio::test]
async fn a_replay_deletes_the_copy_live_extraction_had_skipped() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let wrong = "User works at Acme Corp";
    let source = MemorySource::Event { id: Uuid::new_v4() };
    let event = recorded(&pool, wrong, &[], &source).await;

    let embedder = &BagOfWordsEmbedder;
    let older = event.created - Duration::hours(1);
    let echo = MemorySource::Event { id: Uuid::new_v4() };
    let copy = seed(&index, embedder, wrong, &[], &echo, older).await;
    replay_corrections(&index, embedder, &[event]).await;

    assert!(index.get_by_id(copy).await.unwrap().is_none());

    teardown_test_db(&db_name).await;
}

/// Live extraction also supersedes a close entry that shares an entity, so the
/// correction names only the survivor. A rebuild can write both, and the
/// superseded copy says the same wrong thing.
#[tokio::test]
async fn a_replay_deletes_the_copy_live_extraction_had_superseded() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let wrong = "User lives in Bergen";
    let bergen = vec!["Bergen".to_string()];
    let event = recorded(
        &pool,
        wrong,
        &bergen,
        &MemorySource::Event { id: Uuid::new_v4() },
    )
    .await;

    let embedder = &BagOfWordsEmbedder;
    let older = event.created - Duration::hours(1);
    let close = "User lives in Bergen now";
    let other = MemorySource::Event { id: Uuid::new_v4() };
    let superseded = seed(&index, embedder, close, &bergen, &other, older).await;
    let unrelated = seed(&index, embedder, close, &[], &other, older).await;
    replay_corrections(&index, embedder, &[event]).await;

    assert!(index.get_by_id(superseded).await.unwrap().is_none());
    assert!(
        index.get_by_id(unrelated).await.unwrap().is_some(),
        "extraction supersedes only across a shared entity"
    );

    teardown_test_db(&db_name).await;
}

/// One extraction yields several facts and never weighs them against each
/// other, so a close sibling stood beside the removed fact. The replay takes
/// only the closest entry from that source.
#[tokio::test]
async fn a_replay_keeps_a_sibling_fact_from_the_same_message() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let wrong = "User lives in Bergen";
    let bergen = vec!["Bergen".to_string()];
    let source = MemorySource::Event { id: Uuid::new_v4() };
    let event = recorded(&pool, wrong, &bergen, &source).await;

    let embedder = &BagOfWordsEmbedder;
    let older = event.created - Duration::hours(1);
    let re_extracted = seed(&index, embedder, wrong, &bergen, &source, older).await;
    let sibling = seed(
        &index,
        embedder,
        "User lives in Bergen now",
        &bergen,
        &source,
        older,
    )
    .await;
    replay_corrections(&index, embedder, &[event]).await;

    assert!(index.get_by_id(re_extracted).await.unwrap().is_none());
    assert!(index.get_by_id(sibling).await.unwrap().is_some());

    teardown_test_db(&db_name).await;
}

/// An incremental rebuild skips a source that still has a row, so the removed
/// fact is not re-extracted and only its sibling is left. The sibling was there
/// when the user corrected, and every rebuild must leave it alone.
#[tokio::test]
async fn an_incremental_replay_keeps_what_was_there_at_the_correction() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let bergen = vec!["Bergen".to_string()];
    let source = MemorySource::Event { id: Uuid::new_v4() };

    let embedder = &BagOfWordsEmbedder;
    let older = Utc::now() - Duration::hours(1);
    let sibling = seed(
        &index,
        embedder,
        "User lives in Bergen now",
        &bergen,
        &source,
        older,
    )
    .await;
    let event = recorded(&pool, "User lives in Bergen", &bergen, &source).await;
    replay_corrections(&index, embedder, &[event]).await;

    assert!(index.get_by_id(sibling).await.unwrap().is_some());

    teardown_test_db(&db_name).await;
}

/// A correction's replacement never went through extraction's dedup, so it can
/// stand beside a near-copy the user later removes. That later correction must
/// not take the earlier replacement with it.
#[tokio::test]
async fn a_replay_keeps_an_earlier_corrections_replacement() {
    let (pool, db_name) = setup_test_db().await;
    let index = PgVectorIndex::new(pool.clone()).await.unwrap();
    let text = "User favourite colour is blue";
    let event = recorded(
        &pool,
        text,
        &[],
        &MemorySource::Event { id: Uuid::new_v4() },
    )
    .await;

    let embedder = &BagOfWordsEmbedder;
    let earlier = Uuid::new_v4();
    index_correction(
        &index,
        embedder,
        earlier,
        text,
        event.created - Duration::hours(1),
    )
    .await
    .unwrap();
    replay_corrections(&index, embedder, &[event]).await;

    let replacement = serde_json::to_value(MemorySource::Event { id: earlier }).unwrap();
    assert_eq!(
        index.entries_for_source(&replacement).await.unwrap().len(),
        1
    );

    teardown_test_db(&db_name).await;
}

/// Rows the retired factory wrote carry the payload flat, with no `data`
/// envelope and no sources. They are still the user's corrections.
#[test]
fn a_flat_payload_from_the_retired_factory_still_reads() {
    let row = |payload: serde_json::Value| EventRow::new(MEMORY_CORRECTED, payload);
    let fields = serde_json::json!({ "deleted_summaries": ["X"], "correction": "Y" });

    for payload in [
        fields.clone(),
        serde_json::json!({ "type": MEMORY_CORRECTED, "data": fields }),
    ] {
        let recorded = RecordedCorrection::from_row(&row(payload)).unwrap();
        assert!(recorded.removed.is_empty());
        assert_eq!(recorded.deleted_summaries, vec!["X".to_string()]);
        assert_eq!(recorded.correction.as_deref(), Some("Y"));
    }
}
