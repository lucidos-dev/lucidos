use serde_json::{json, Value};
use uuid::Uuid;

use super::{age_in_days, keywords_for, relevance_score, KEYWORD_BOOST, KEYWORD_SIMILARITY_PROXY};
use crate::engine::LucidosEngine;
use crate::memory::{
    EmbeddingProvider, MemoryEntry, MemorySource, RETRIEVAL_MIN_IMPORTANCE,
    RETRIEVAL_MIN_SIMILARITY,
};

const SEARCH_MAX: i64 = 20;
const SEARCH_DEFAULT: i64 = 10;

const SEARCH_CANDIDATES: usize = 60;

impl LucidosEngine {
    pub async fn search_memory_ranked(
        &self,
        q: &str,
        limit: Option<i64>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let q = q.trim();
        if q.is_empty() {
            return Err("q is required".into());
        }
        let Some(ref index) = self.memory_index else {
            return Err("memory index not available".into());
        };
        let limit = limit.unwrap_or(SEARCH_DEFAULT).clamp(1, SEARCH_MAX) as usize;

        let now = chrono::Utc::now();
        let mut scored: std::collections::HashMap<Uuid, (MemoryEntry, f64)> =
            std::collections::HashMap::new();

        // INVARIANT: Two DIFFERENT questions, and conflating them gets one of them wrong
        // in each direction.
        //
        // `degraded` answers "may this answer be incomplete?", and any failure
        // makes it yes. That is the whole point of the field: this tool is the
        // backstop behind LOOK_BEFORE_ASSESSING_RULE, which licenses the agent
        // to treat a look that found nothing as proven absence, so a partial
        // failure presented as a complete search recreates the 2026-08-09
        // failure through the fix for it. One lookup out of three failing may
        // be exactly the one that held the answer.
        //
        // `*_ran` answers "did anything run at all?", and only a total failure
        // makes it no. That one gates the hard error below, so a search that
        // DID return hits is never thrown away and never reported as "neither
        // index answered".
        let mut degraded: Vec<&str> = Vec::new();
        let mut semantic_ran = false;

        match self.embedder.embed_batch(&[q]).await {
            Ok(embeddings) => match embeddings.first().map(Vec::as_slice) {
                Some(embedding) => match index
                    .search_with_scores(
                        embedding,
                        RETRIEVAL_MIN_IMPORTANCE,
                        SEARCH_CANDIDATES,
                        self.embedder.model_id(),
                    )
                    .await
                {
                    Ok(hits) => {
                        semantic_ran = true;
                        for (entry, similarity) in hits {
                            if similarity < RETRIEVAL_MIN_SIMILARITY {
                                continue;
                            }
                            let score = relevance_score(
                                similarity,
                                entry.importance,
                                age_in_days(now, entry.src_created_at),
                            );
                            scored.insert(entry.id, (entry, score));
                        }
                    }
                    Err(e) => {
                        crate::log_legacy!(@Memory, "search: semantic arm failed: {}", e);
                        degraded.push("semantic");
                    }
                },
                None => degraded.push("semantic"),
            },
            Err(e) => {
                crate::log_legacy!(@Memory, "search: embedding failed, keyword only: {}", e);
                degraded.push("semantic");
            }
        }

        // INVARIANT: Keyword arm, one lookup PER WORD. `search_by_keyword` matches
        // `summary ILIKE '%needle%'`, so handing it the whole question asks for
        // that exact substring and finds nothing: the arm was dead on every
        // multi-word query, which is every query the schema asks for. Same
        // tokenizer and same scoring the injection uses.
        let keyword_hits = futures::future::join_all(
            keywords_for([q])
                .iter()
                .map(|kw| index.search_by_keyword(kw, RETRIEVAL_MIN_IMPORTANCE, SEARCH_CANDIDATES))
                .collect::<Vec<_>>(),
        )
        .await;
        // INVARIANT: At most once per entry, however many of its words matched, exactly as
        // `retrieve_context` does it: the boost says "keyword found this", not
        // "keyword found this five times".
        let mut boosted: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let (mut keyword_ran, mut keyword_failed) = (false, false);
        for result in keyword_hits {
            match result {
                Ok(result) => {
                    keyword_ran = true;
                    for entry in result.entries {
                        if !boosted.insert(entry.id) {
                            continue;
                        }
                        scored
                            .entry(entry.id)
                            // INVARIANT: The SAME formula the injection uses: an entry the
                            // semantic arm already scored keeps that score and
                            // gains the boost, rather than being compared
                            // against a flat proxy. Taking `max` instead
                            // inverted pairs against the injection (a strong
                            // semantic hit that is also a keyword hit ranked
                            // BELOW a slightly stronger semantic-only one),
                            // which is the two-orderings problem this module
                            // says cannot happen.
                            .and_modify(|(_, existing)| *existing *= KEYWORD_BOOST)
                            .or_insert_with(|| {
                                let score = relevance_score(
                                    KEYWORD_SIMILARITY_PROXY,
                                    entry.importance,
                                    age_in_days(now, entry.src_created_at),
                                ) * KEYWORD_BOOST;
                                (entry, score)
                            });
                    }
                }
                Err(e) => {
                    crate::log_legacy!(@Memory, "search: keyword arm failed: {}", e);
                    keyword_failed = true;
                }
            }
        }
        // INVARIANT: ANY failed lookup degrades the arm, even beside successful ones: the
        // word that failed may be the discriminating one, so what came back is
        // not the whole answer and must not read as though it were.
        if keyword_failed {
            degraded.push("keyword");
        }

        let mut ranked: Vec<(MemoryEntry, f64)> = scored.into_values().collect();
        ranked.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let total_matched = ranked.len();
        ranked.truncate(limit);

        // INVARIANT: Nothing ran anywhere is not a search, so it is an error rather than
        // an empty page: an empty page is indistinguishable from "nothing
        // matched", and the agent is told it may act on that. Keyed on `*_ran`
        // rather than on `degraded`, so a partially-degraded search that still
        // returned hits keeps them.
        if !semantic_ran && !keyword_ran {
            return Err(
                "memory search could not run: neither the semantic nor the keyword \
                        index answered. This is NOT evidence that nothing matched."
                    .into(),
            );
        }

        // INVARIANT: Built rather than inlined: `json!` with a `None` emits the key as
        // `null`, so the field would be present on every ordinary search and
        // the "absent means complete" contract would be a lie.
        let mut out = serde_json::Map::new();
        if !degraded.is_empty() {
            out.insert("degraded".into(), json!(degraded));
        }
        out.insert(
            "results".into(),
            json!(ranked
                .into_iter()
                .map(|(entry, score)| json!({
                    "id": entry.id,
                    "date": entry.src_created_at.format("%Y-%m-%d").to_string(),
                    "topic": entry.topic,
                    "summary": entry.summary,
                    "importance": entry.importance,
                    "score": score,
                    "source": entry.source,
                }))
                .collect::<Vec<_>>()),
        );
        out.insert("total_matched".into(), json!(total_matched));
        Ok(Value::Object(out))
    }

    pub async fn memory_source(
        &self,
        id: Uuid,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref index) = self.memory_index else {
            return Err("memory index not available".into());
        };

        let event_id = match index.get_by_id(id).await? {
            Some(entry) => match entry.source {
                MemorySource::Event { id } => id,
                MemorySource::Artifact { path, commit } => {
                    return Err(format!(
                        "memory {id} came from the artifact {path} at {commit}, not from a \
                         conversation, so there is no thread to open"
                    )
                    .into())
                }
            },
            None => id,
        };

        let event = self.event_store().get_event_by_id(event_id).await?;
        let entries = index
            .entries_for_source(&json!({"type": "event", "id": event_id.to_string()}))
            .await?;

        Ok(json!({
            "source_type": "event",
            "event": event.map(|e| json!({
                "id": e.id,
                "event_type": e.event_type,
                "thread_id": e.thread_id,
                "payload": e.payload,
                "created": e.created,
            })),
            "entries": entries,
        }))
    }
}
