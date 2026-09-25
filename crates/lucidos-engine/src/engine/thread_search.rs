//! Thread search: text plus semantic merge, with the score combination and
//! dampening that keeps a focused thread above a catch-all, and the ranking
//! that puts title hits first and newer hits above older ones.
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

/// How much of the query a thread's title holds as one piece. Declared weakest
/// first, so the derived `Ord` ranks an exact title above everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TitleMatch {
    None,
    /// The title contains the whole query as a phrase.
    Phrase,
    /// The title is the query.
    Exact,
}

/// Case- and whitespace-insensitive, so "Fix  Search" is an exact hit for
/// "fix search".
pub(super) fn title_match(title: &str, query: &str) -> TitleMatch {
    fn normalize(s: &str) -> String {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
    let (title, query) = (normalize(title), normalize(query));
    if query.is_empty() {
        TitleMatch::None
    } else if title == query {
        TitleMatch::Exact
    } else if title.contains(&query) {
        TitleMatch::Phrase
    } else {
        TitleMatch::None
    }
}

/// Scale a score by age: `score * (0.5 + 0.5 * exp(-age_days / 14))`. A hit
/// from today keeps its score and a months-old one keeps half. Items with no
/// timestamp keep their score.
pub(crate) fn recency_boost(
    score: f64,
    last_activity: Option<chrono::DateTime<chrono::Utc>>,
) -> f64 {
    match last_activity {
        Some(ts) => {
            let age_days = (chrono::Utc::now() - ts).num_seconds() as f64 / 86400.0;
            score * (0.5 + 0.5 * (-age_days / 14.0).exp())
        }
        None => score,
    }
}

/// The text score a hit ranks by. Only a hit outside the title is dampened: a
/// title naming the query is no incidental keyword, however long the thread.
pub(super) fn text_hit_score(score: f64, message_count: i64, title: TitleMatch) -> f64 {
    match title {
        TitleMatch::None => dampen_text_score(score, message_count),
        TitleMatch::Phrase | TitleMatch::Exact => score,
    }
}

/// What a hit ranks by, strongest field first. The first two are hard tiers.
pub(super) struct RankKey {
    /// An old thread titled with the query outranks a fresh one that only
    /// mentions it.
    pub(super) title: TitleMatch,
    /// The text arm found it. Recency must never lift a meaning-only hit over
    /// a real word match, the promise `SEMANTIC_WEIGHT` exists to keep.
    pub(super) text_hit: bool,
    /// Already recency-boosted.
    pub(super) score: f64,
    pub(super) last_activity: chrono::DateTime<chrono::Utc>,
}

/// Best first.
pub(super) fn rank_order(a: &RankKey, b: &RankKey) -> std::cmp::Ordering {
    b.title
        .cmp(&a.title)
        .then_with(|| b.text_hit.cmp(&a.text_hit))
        .then_with(|| b.score.total_cmp(&a.score))
        .then_with(|| b.last_activity.cmp(&a.last_activity))
}

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
/// text matches always rank above pure semantic noise (see `combined_score`),
/// then ranks by `rank_order`: title hits, then word hits, then meaning-only
/// hits, with newer hits boosted within each. The returned scores carry that
/// boost.
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
                engine.embedder.model_id(),
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
    let text_hit_count = text_results.len();

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged = Vec::new();
    for mut r in text_results {
        r.score = text_hit_score(r.score, r.info.message_count, title_match(&r.info.title, q));
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

    // The text hits are the first `text_hit_count` entries: semantic-only hits
    // were appended after them.
    let mut ranked: Vec<_> = merged
        .into_iter()
        .enumerate()
        .map(|(idx, mut r)| {
            r.score = recency_boost(r.score, Some(r.info.last_activity));
            let key = RankKey {
                title: title_match(&r.info.title, q),
                text_hit: idx < text_hit_count,
                score: r.score,
                last_activity: r.info.last_activity,
            };
            (key, r)
        })
        .collect();
    ranked.sort_by(|(a, _), (b, _)| rank_order(a, b));
    // Deliberately NOT truncated to `limit` here. `limit` bounds each ARM, so
    // two arms that agree on nothing return up to 2 x limit between them. The
    // callers that PROMISE a maximum apply it themselves, which is the only
    // place the promise exists.
    Ok(ranked.into_iter().map(|(_, r)| r).collect())
}

#[cfg(test)]
#[path = "thread_search_tests.rs"]
mod tests;
