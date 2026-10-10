//! The `recall` tool, the Tree memory module's four recall tools (ADR 0362):
//! `zoom`, `find`, `search` and `date`. The routes in `api/recall.rs` and the
//! generated `lucidos recall` commands call the same engine methods.
//!
//! `find` walks the workspace tree with the `MemoryFind` judgment site, and
//! batches rows the way pg-jev does. A text prefilter caps each round, and
//! about 40 lines go in one request. Batches run in parallel, and each verdict
//! is cached by query and line.

use std::collections::HashMap;
use std::sync::Mutex;

use futures::future::join_all;
use serde::Serialize;
use uuid::Uuid;

use crate::engine::summary_tree::fold::built_cover;
use crate::engine::summary_tree::recall::{self, RecallLine};
use crate::engine::summary_tree::store;
use crate::engine::summary_tree::view::NodeId;
use crate::engine::summary_tree::{NodeAddr, SummaryScope};
use crate::engine::{AuxCapture, ContextPurpose, LucidosEngine};
use crate::llm::judgment::{
    for_site, system_one_for, ChatJudgmentProvider, JudgmentProvider, JudgmentSite, NoulCriteria,
    Question,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Lines in one judgment request, as pg-jev batches rows.
const FIND_BATCH: usize = 40;
/// Lines one round may judge. The text prefilter keeps the likeliest.
const FIND_ROUND_CAP: usize = 200;
/// Lines one round opens further.
const FIND_OPEN: usize = 8;
/// Levels one round opens a line by.
const FIND_LEVELS: u32 = 2;
/// Rounds before the walk stops.
const FIND_ROUNDS: usize = 8;
/// After this long no new round starts, so a slow judge returns what it found
/// inside a CLI or tool call's patience.
const FIND_DEADLINE: std::time::Duration = std::time::Duration::from_secs(25);
/// The probability at or above which a line reads as relevant.
const FIND_THRESHOLD: f64 = 0.5;
/// Verdicts the cache keeps before it starts over.
const FIND_CACHE_ENTRIES: usize = 20_000;

/// Results a search or find returns unless asked otherwise, and the most.
const DEFAULT_RESULTS: usize = 10;
const MAX_RESULTS: usize = 20;

/// Cached `find` verdicts, keyed by query and line. A line's text is part of
/// the key, so a rebuilt node is judged afresh.
#[derive(Default)]
pub(crate) struct FindCache(Mutex<HashMap<(String, String, String), f64>>);

impl FindCache {
    fn get(&self, query: &str, line: &RecallLine) -> Option<f64> {
        let key = (query.to_string(), line.id.clone(), line.text.clone());
        self.0.lock().unwrap().get(&key).copied()
    }

    fn put(&self, query: &str, line: &RecallLine, p: f64) {
        let mut cache = self.0.lock().unwrap();
        if cache.len() >= FIND_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert((query.to_string(), line.id.clone(), line.text.clone()), p);
    }
}

/// One `find` result.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct FindHit {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) probability: f64,
}

fn limit_arg(args: &serde_json::Value) -> usize {
    args.get("limit")
        .and_then(|v| v.as_u64())
        .map_or(DEFAULT_RESULTS, |n| (n as usize).clamp(1, MAX_RESULTS))
}

fn str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The `id` argument, a bare `start+span` naming the calling thread.
fn id_arg(args: &serde_json::Value, thread_id: Uuid) -> Result<NodeId, String> {
    let id = str_arg(args, "id").ok_or("id is required: a line's id from a memory view.")?;
    NodeId::parse(id, Some(thread_id))
}

impl LucidosEngine {
    pub(crate) async fn execute_recall_zoom(
        &self,
        args: &serde_json::Value,
        thread_id: Uuid,
    ) -> Result<String, BoxError> {
        let id = match id_arg(args, thread_id) {
            Ok(id) => id,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let levels = args.get("n").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        Ok(serde_json::to_string(&self.recall_zoom(id, levels).await?)?)
    }

    pub(crate) async fn execute_recall_date(
        &self,
        args: &serde_json::Value,
        thread_id: Uuid,
    ) -> Result<String, BoxError> {
        let id = match id_arg(args, thread_id) {
            Ok(id) => id,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        Ok(serde_json::to_string(&self.recall_date(id).await?)?)
    }

    pub(crate) async fn execute_recall_search(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, BoxError> {
        let Some(text) = str_arg(args, "text") else {
            return Ok("Error: text is required: words the messages hold.".to_string());
        };
        let hits = self.recall_search(text, limit_arg(args)).await?;
        Ok(serde_json::to_string(&hits)?)
    }

    pub(crate) async fn execute_recall_find(
        &self,
        args: &serde_json::Value,
        thread_id: Uuid,
    ) -> Result<String, BoxError> {
        let Some(query) = str_arg(args, "query") else {
            return Ok("Error: query is required: what to find, in plain words.".to_string());
        };
        let hits = self
            .recall_find(query, limit_arg(args), Some(thread_id))
            .await?;
        Ok(serde_json::to_string(&hits)?)
    }

    /// `zoom`: the lines under `id`, `levels` levels down.
    pub(crate) async fn recall_zoom(
        &self,
        id: NodeId,
        levels: u32,
    ) -> Result<serde_json::Value, BoxError> {
        let lines = self.recall_lines(id, levels).await?;
        Ok(serde_json::json!({ "id": id.to_string(), "lines": lines }))
    }

    async fn recall_lines(&self, id: NodeId, levels: u32) -> Result<Vec<RecallLine>, BoxError> {
        let artifacts = &self.artifact_manager;
        let read =
            |path: &str, commit: &str| artifacts.read_artifact_at_commit_string(path, commit).ok();
        recall::zoom(&self.pool, id, levels, &read).await
    }

    /// `date`: when the entries under `id` happened.
    pub(crate) async fn recall_date(&self, id: NodeId) -> Result<serde_json::Value, BoxError> {
        recall::date(&self.pool, id).await
    }

    /// `search`: messages holding every word of `text`, with tree addresses.
    pub(crate) async fn recall_search(
        &self,
        text: &str,
        limit: usize,
    ) -> Result<Vec<recall::SearchHit>, BoxError> {
        recall::search(
            &self.pool,
            &self.event_store,
            text,
            limit.clamp(1, MAX_RESULTS),
        )
        .await
    }

    /// `find`: walk the workspace tree for lines bearing on `query`.
    ///
    /// Each round judges a frontier of lines and opens the likeliest further,
    /// until it reaches messages. A thread leaf is a result, and so is a line
    /// that opens into nothing finer, such as an artifact. `cost_thread`
    /// files the judgment calls' cost; `None` files it on the home thread.
    pub(crate) async fn recall_find(
        &self,
        query: &str,
        limit: usize,
        cost_thread: Option<Uuid>,
    ) -> Result<Vec<FindHit>, BoxError> {
        let judge = self.find_judge().await?;
        let capture = AuxCapture::for_thread_or_home(
            &self.event_bus,
            cost_thread,
            ContextPurpose::MemoryFind,
        );
        let mut frontier = self.find_roots().await?;
        let mut hits: Vec<FindHit> = Vec::new();
        let started = std::time::Instant::now();

        for _ in 0..FIND_ROUNDS {
            if frontier.is_empty() || started.elapsed() >= FIND_DEADLINE {
                break;
            }
            let rows = prefilter(query, frontier, FIND_ROUND_CAP);
            let verdicts = self
                .judge_rows(judge.as_ref(), query, &rows, &capture)
                .await;
            let mut kept: Vec<(RecallLine, f64)> = rows
                .into_iter()
                .zip(verdicts)
                .filter(|(_, p)| *p >= FIND_THRESHOLD)
                .collect();
            kept.sort_by(|a, b| b.1.total_cmp(&a.1));

            let mut next = Vec::new();
            for (line, p) in kept.into_iter().take(FIND_OPEN) {
                let Ok(id) = NodeId::parse(&line.id, None) else {
                    continue;
                };
                if matches!(id.scope, SummaryScope::Thread(_)) && id.addr.is_leaf() {
                    hits.push(hit(line, p));
                    continue;
                }
                let opened = self.recall_lines(id, FIND_LEVELS).await?;
                if opened.len() == 1 && opened[0].id == line.id {
                    hits.push(hit(opened.into_iter().next().expect("one line"), p));
                } else {
                    next.extend(opened);
                }
            }
            frontier = next;
        }
        hits.sort_by(|a, b| b.probability.total_cmp(&a.probability));
        hits.dedup_by(|a, b| a.id == b.id);
        hits.truncate(limit.clamp(1, MAX_RESULTS));
        Ok(hits)
    }

    /// The site's provider: a System One pick with chat behind it, or chat.
    async fn find_judge(&self) -> Result<Box<dyn JudgmentProvider>, BoxError> {
        let call = self.aux_call(ContextPurpose::MemoryFind).await;
        let system_one =
            system_one_for(&self.pool, JudgmentSite::MemoryFind, call.attempt_timeout()).await;
        let chat = ChatJudgmentProvider::new(call.provider(), call.reasoning().map(str::to_string));
        Ok(for_site(system_one, chat))
    }

    /// Where a walk starts: the workspace tree's coarsest built lines, split
    /// from the newest end until about one batch of them.
    async fn find_roots(&self) -> Result<Vec<RecallLine>, BoxError> {
        let shapes = store::node_shapes(&self.pool, SummaryScope::Workspace).await?;
        let end = (0..)
            .find(|i| !shapes.contains_key(&NodeAddr::leaf(*i)))
            .unwrap_or(0);
        let built = |a: NodeAddr| shapes.get(&a).map(|s| s.bytes);
        let mut tiles = built_cover(end, built);
        while tiles.len() < FIND_BATCH {
            let Some(pos) = tiles.iter().rposition(|t| {
                t.children()
                    .is_some_and(|(a, b)| built(a).is_some() && built(b).is_some())
            }) else {
                break;
            };
            let (a, b) = tiles[pos].children().expect("checked above");
            tiles.splice(pos..=pos, [a, b]);
        }
        let nodes = store::nodes_at(&self.pool, SummaryScope::Workspace, &tiles).await?;
        Ok(tiles
            .into_iter()
            .filter_map(|t| {
                nodes.get(&t).map(|n| RecallLine {
                    id: format!("w/{t}"),
                    text: n.text.clone(),
                })
            })
            .collect())
    }

    /// Each row's probability of bearing on `query`: cached verdicts first,
    /// the rest in parallel batches. A batch that fails reads as no, so one
    /// bad call narrows the walk rather than ending it.
    async fn judge_rows(
        &self,
        judge: &dyn JudgmentProvider,
        query: &str,
        rows: &[RecallLine],
        capture: &AuxCapture,
    ) -> Vec<f64> {
        let mut verdicts: Vec<Option<f64>> =
            rows.iter().map(|r| self.find_cache.get(query, r)).collect();
        let pending: Vec<usize> = (0..rows.len()).filter(|&i| verdicts[i].is_none()).collect();
        let deadline = crate::engine::aux_purpose::budget_for(ContextPurpose::MemoryFind).deadline;
        let batches = pending
            .chunks(FIND_BATCH)
            .map(|batch| judge_batch(judge, query, rows, batch, deadline, capture));
        for (i, p) in join_all(batches).await.into_iter().flatten() {
            self.find_cache.put(query, &rows[i], p);
            verdicts[i] = Some(p);
        }
        verdicts.into_iter().map(|v| v.unwrap_or(0.0)).collect()
    }
}

/// One request: every row in `batch` as a yes/no question over one state.
async fn judge_batch(
    judge: &dyn JudgmentProvider,
    query: &str,
    rows: &[RecallLine],
    batch: &[usize],
    deadline: std::time::Duration,
    capture: &AuxCapture,
) -> Vec<(usize, f64)> {
    let state = serde_json::json!({
        "query": query,
        "lines": batch.iter().map(|&i| serde_json::json!({
            "line": format!("l{i}"),
            "text": rows[i].text,
        })).collect::<Vec<_>>(),
    });
    let questions = batch
        .iter()
        .map(|&i| (format!("l{i}"), line_question(i)))
        .collect();
    let bounded = capture.until(tokio::time::Instant::now() + deadline);
    let judgment = match bounded.judge(judge, state, questions).await {
        Ok(judgment) => judgment,
        Err(e) if e.is::<tokio::time::error::Elapsed>() => {
            log!("[Recall] A find batch ran out of time, so its lines read as no");
            return Vec::new();
        }
        Err(e) => {
            log!(
                "[Recall] A find batch failed, so its lines read as no: {}",
                e
            );
            return Vec::new();
        }
    };
    batch
        .iter()
        .filter_map(|&i| Some((i, judgment.answers.noul(&format!("l{i}"))?)))
        .collect()
}

fn line_question(i: usize) -> Question {
    Question::Noul {
        instructions: format!(
            "Does line `l{i}` in `lines` cover something that answers or bears on `query`? \
             A summary line covers more than it says, so judge what it could contain."
        ),
        criteria: Some(NoulCriteria {
            yes: "The line names the subject of the query, or a conversation or file where \
                  it would plausibly come up."
                .to_string(),
            no: "The line is about something else entirely.".to_string(),
        }),
    }
}

fn hit(line: RecallLine, probability: f64) -> FindHit {
    FindHit {
        id: line.id,
        text: line.text,
        probability,
    }
}

/// The text prefilter: past `cap` rows, keep those sharing the most words
/// with the query. Order among the kept rows stays the tree's.
fn prefilter(query: &str, rows: Vec<RecallLine>, cap: usize) -> Vec<RecallLine> {
    if rows.len() <= cap {
        return rows;
    }
    let words: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3)
        .collect();
    let overlap = |line: &RecallLine| {
        let text = line.text.to_lowercase();
        words.iter().filter(|w| text.contains(w.as_str())).count()
    };
    let mut ranked: Vec<(usize, usize)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (overlap(r), i))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut keep: Vec<usize> = ranked.into_iter().take(cap).map(|(_, i)| i).collect();
    keep.sort_unstable();
    let mut rows: Vec<Option<RecallLine>> = rows.into_iter().map(Some).collect();
    keep.into_iter().filter_map(|i| rows[i].take()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, text: &str) -> RecallLine {
        RecallLine {
            id: id.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn the_prefilter_keeps_the_rows_sharing_the_query_words_in_tree_order() {
        let rows = vec![
            line("w/0+1", "groceries"),
            line("w/1+1", "the boat engine repair"),
            line("w/2+1", "weather"),
            line("w/3+1", "engine oil for the boat"),
        ];
        let kept = prefilter("boat engine", rows, 2);
        let ids: Vec<&str> = kept.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["w/1+1", "w/3+1"]);
    }

    #[test]
    fn under_the_cap_every_row_is_judged() {
        let rows = vec![line("w/0+1", "a"), line("w/1+1", "b")];
        assert_eq!(prefilter("zzz", rows, 40).len(), 2);
    }

    #[test]
    fn a_cached_verdict_is_keyed_by_the_line_text() {
        let cache = FindCache::default();
        cache.put("q", &line("w/0+1", "old text"), 0.9);
        assert_eq!(cache.get("q", &line("w/0+1", "old text")), Some(0.9));
        assert_eq!(cache.get("q", &line("w/0+1", "rebuilt text")), None);
        assert_eq!(cache.get("other", &line("w/0+1", "old text")), None);
    }
}
