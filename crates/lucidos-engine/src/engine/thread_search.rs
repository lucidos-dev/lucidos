//! Thread search: text plus semantic merge, with the score combination and
//! dampening that keeps a focused thread above a catch-all.
//!
//! Lives on the engine rather than behind the HTTP handler because it has two
//! consumers with nothing else in common. `/api/v1/threads/search` and the
//! SearchEverywhere endpoint serve the UI's search box; the grouped `threads`
//! tool's `search` action serves the agent, which is how it answers "we talked
//! about this" (`list` filters by status and channel and can never find a
//! topic). Only conversational data crosses HTTP in this codebase, so the
//! agent path calls straight in here rather than to a route.

use std::collections::HashMap;
use uuid::Uuid;

use crate::engine::LucidosEngine;
use crate::memory::{
    EmbeddingProvider, MemorySource, RETRIEVAL_MIN_IMPORTANCE, RETRIEVAL_MIN_SIMILARITY,
};

/// Weight applied to semantic similarity when combining with a text score.
/// Chosen so that `SEMANTIC_WEIGHT * 1.0` (best possible semantic) stays
/// strictly below the 0.7 text content-match floor in `search_threads_by_text`,
/// keeping pure-semantic noise from outranking legitimate keyword hits.
const SEMANTIC_WEIGHT: f64 = 0.5;

/// Threads at or below this size keep their full text-match score. Larger
/// catch-all threads (e.g. one that accidentally accumulates 2000+ events of
/// unrelated work) get score scaled by `threshold / count`: hyperbolic decay
/// so a couple of incidental keyword matches don't put them above focused
/// thematic threads.
pub(super) const TEXT_MATCH_DAMPEN_THRESHOLD: i64 = 100;

/// Upper bound on how many memory events the semantic search returns before
/// thread-level aggregation. Multilingual-e5 has a ~0.85 cosine noise floor
/// so a top-N of 20 fills with full-concept matches and crowds out partial
/// matches (e.g. a thread with separate father-only and eye-only facts).
/// Aggregation downstream picks the best event per thread, so a wider set
/// gives such threads a chance to surface.
const SEMANTIC_CANDIDATE_LIMIT: usize = 200;

/// Reduce a text-match score for threads larger than the threshold so a few
/// incidental keyword hits in a giant catch-all thread can't outrank a
/// focused short thread that's actually about the topic.
pub(super) fn dampen_text_score(score: f64, message_count: i64) -> f64 {
    let count = message_count.max(1) as f64;
    let threshold = TEXT_MATCH_DAMPEN_THRESHOLD as f64;
    if count <= threshold {
        score
    } else {
        score * (threshold / count)
    }
}

/// Combine text and semantic similarity into a single ranking score.
///
/// Text matches always outrank pure semantic noise: multilingual-e5-small
/// produces a ~0.85+ similarity floor for any query, so a raw MAX would let
/// unrelated threads outscore legitimate keyword hits.
pub(super) fn combined_score(text: Option<f64>, semantic: Option<f64>) -> f64 {
    match (text, semantic) {
        (Some(t), Some(s)) => t + SEMANTIC_WEIGHT * s,
        (Some(t), None) => t,
        (None, Some(s)) => SEMANTIC_WEIGHT * s,
        (None, None) => 0.0,
    }
}

/// Run text + semantic thread search and merge results. Combines scores so
/// text matches always rank above pure semantic noise (see `combined_score`).
/// Empty/whitespace queries return an empty vec. Used by both
/// `/api/v1/threads/search` and the SearchEverywhere `/api/v1/search` endpoint.
pub(crate) async fn combined_thread_search(
    engine: &LucidosEngine,
    query: &str,
    limit: i64,
) -> Result<Vec<crate::core::store::ThreadSearchResult>, Box<dyn std::error::Error + Send + Sync>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let store = engine.event_store();

    let semantic_future = async {
        let index = engine.memory_index.as_ref()?;
        let embedding = match engine.embedder.embed(q).await {
            Ok(e) => e,
            Err(e) => {
                log!("[Search] embedder failed, degrading to text-only: {}", e);
                return None;
            }
        };
        let scored_results = match index
            .search_with_scores(
                &embedding,
                RETRIEVAL_MIN_IMPORTANCE,
                SEMANTIC_CANDIDATE_LIMIT,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log!(
                    "[Search] semantic search failed, degrading to text-only: {}",
                    e
                );
                return None;
            }
        };
        let scored_event_ids: Vec<(Uuid, f64)> = scored_results
            .iter()
            .filter(|(_, similarity)| *similarity >= RETRIEVAL_MIN_SIMILARITY)
            .filter_map(|(entry, similarity)| match &entry.source {
                MemorySource::Event { id } => Some((*id, *similarity)),
                _ => None,
            })
            .collect();
        match store
            .search_threads_by_memory(&scored_event_ids, limit)
            .await
        {
            Ok(r) => Some(r),
            Err(e) => {
                log!(
                    "[Search] thread aggregation failed, degrading to text-only: {}",
                    e
                );
                None
            }
        }
    };

    let (text_result, semantic_result) =
        tokio::join!(store.search_threads_by_text(q, limit), semantic_future);
    let text_results = text_result?;

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged = Vec::new();
    for mut r in text_results {
        r.score = dampen_text_score(r.score, r.info.message_count);
        seen.insert(r.info.thread_id.clone(), merged.len());
        merged.push(r);
    }
    if let Some(semantic) = semantic_result {
        for mut r in semantic {
            if let Some(&idx) = seen.get(&r.info.thread_id) {
                merged[idx].score = combined_score(Some(merged[idx].score), Some(r.score));
            } else {
                r.score = combined_score(None, Some(r.score));
                seen.insert(r.info.thread_id.clone(), merged.len());
                merged.push(r);
            }
        }
    }

    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.info.last_activity.cmp(&a.info.last_activity))
    });
    // Deliberately NOT truncated to `limit` here. `limit` bounds each ARM, so
    // two arms that agree on nothing return up to 2 x limit between them, and
    // SearchEverywhere depends on that: it asks for 5 per category and then
    // re-scores the merge with a recency boost before taking its own slice
    // (`api::search`), so cutting here would drop threads the boost was about
    // to lift. The callers that PROMISE a maximum apply it themselves, which is
    // the only place the promise exists.
    Ok(merged)
}

#[cfg(test)]
#[path = "thread_search_tests.rs"]
mod tests;
