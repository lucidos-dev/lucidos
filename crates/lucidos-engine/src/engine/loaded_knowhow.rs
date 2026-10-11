//! A thread's *loaded knowhow*: the docs its `[LOADED KNOWHOW]` block carries
//! this turn, and how long each one stays (ADR 0425).
//!
//! The set is a pure fold over the thread's events, run once per turn. Events
//! are the authority, so the set survives a restart with nothing to persist.
//!
//! Producers: the `load_knowhow` and `unload_knowhow` tools in
//! `engine/tools/apps.rs`, through the tool pairs they record. Consumers: the
//! block and the viewer rows in `engine/chat/process/context_build.rs`, the
//! history strip in `history.rs`, and the resume stubs in
//! `core/store/messages/resume.rs`.

use crate::core::EventRow;
use crate::engine::LucidosEngine;
use crate::llm::tool_names::{LOAD_KNOWHOW, UNLOAD_KNOWHOW};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

/// How many finished turns in a row may pass without a `load_knowhow` of a
/// doc before the doc leaves the block.
pub const IDLE_TURNS_BEFORE_UNLOAD: usize = 3;

/// The event that ends a finished turn, and so ages every loaded doc. A Stop,
/// an abort or a failure ends a turn that did not finish, and ages nothing.
pub(crate) const TURN_FINISHED_EVENT: &str = "ResponseGenerated";

/// The tools whose pairs change the set.
pub(crate) const KNOWHOW_TOOLS: [&str; 2] = [LOAD_KNOWHOW, UNLOAD_KNOWHOW];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedKnowhow {
    pub id: String,
    /// The formatted `[KNOW-HOW: …]` / `[SYSTEM-KNOWHOW: …]` block, verbatim
    /// as the load returned it.
    pub body: String,
    /// Finished turns since a `load_knowhow` of this doc, kept or full.
    pub idle_turns: usize,
    /// Finished turns since the body itself arrived. Zero means it arrived in
    /// the running turn, so it is in a live tool result rather than the block.
    turns_since_body: usize,
}

impl LoadedKnowhow {
    /// Whether this turn's block holds the body: it arrived before a turn
    /// finished. A load in an unfinished turn reads as not held, so the agent
    /// gets the body again rather than none.
    pub(crate) fn body_in_block(&self) -> bool {
        self.turns_since_body > 0
    }

    /// Why the doc is still here, for the block and the Context viewer row.
    pub(crate) fn retention_note(&self) -> String {
        match IDLE_TURNS_BEFORE_UNLOAD.saturating_sub(self.idle_turns) {
            0 => "leaves after this turn unless loaded again".to_string(),
            1 => "stays 1 more turn after this one unless loaded again".to_string(),
            n => format!("stays {n} more turns after this one unless loaded again"),
        }
    }
}

#[cfg(test)]
impl LoadedKnowhow {
    /// A doc whose body arrived `idle_turns` finished turns ago, unused since.
    pub(crate) fn loaded_turns_ago(id: &str, body: &str, idle_turns: usize) -> Self {
        Self {
            id: id.to_string(),
            body: body.to_string(),
            idle_turns,
            turns_since_body: idle_turns,
        }
    }
}

/// The `load_knowhow` result for a doc this turn's block already holds.
pub(crate) fn kept_result(id: &str) -> String {
    format!(
        "{KEPT_PREFIX}`{id}` is in the [LOADED KNOWHOW] section of this turn's user message, \
         with its body. This call keeps it for {IDLE_TURNS_BEFORE_UNLOAD} more turns."
    )
}

const KEPT_PREFIX: &str = "Already loaded: ";

fn is_kept_result(result: &str) -> bool {
    result.starts_with(KEPT_PREFIX)
}

enum Step {
    Loaded { id: String, body: String },
    Kept { id: String },
    Unloaded { id: String },
}

/// The step one recorded tool pair takes, or `None` for a pair that changes
/// nothing: another tool, an orphan, a failure, a miss, or no id.
fn step_of(pair: crate::core::store::ResumeToolPair) -> Option<(Uuid, Step)> {
    if !KNOWHOW_TOOLS.contains(&pair.tool_name.as_str()) || !pair.succeeded {
        return None;
    }
    let result = pair.result?;
    let id = pair
        .args
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let step = if pair.tool_name == UNLOAD_KNOWHOW {
        Step::Unloaded { id }
    } else if crate::core::knowhow::is_not_found_body(&result) {
        return None;
    } else if is_kept_result(&result) {
        Step::Kept { id }
    } else {
        Step::Loaded { id, body: result }
    };
    Some((pair.tool_called_event_id, step))
}

/// The thread's loaded knowhow as of the end of `events`, sorted by id.
///
/// `events` is the thread's events in order: all of them, or only the
/// shapes [`crate::core::EventStore::get_thread_knowhow_events`] selects.
pub(crate) fn loaded_knowhow(events: &[EventRow]) -> Vec<LoadedKnowhow> {
    let mut steps: HashMap<Uuid, Step> =
        crate::core::store::collect_tool_pairs_chronological(events)
            .into_iter()
            .filter_map(step_of)
            .collect();
    let mut docs: BTreeMap<String, LoadedKnowhow> = BTreeMap::new();
    for event in events {
        if event.event_type == TURN_FINISHED_EVENT {
            for doc in docs.values_mut() {
                doc.idle_turns += 1;
                doc.turns_since_body += 1;
            }
            docs.retain(|_, doc| doc.idle_turns <= IDLE_TURNS_BEFORE_UNLOAD);
            continue;
        }
        match steps.remove(&event.id) {
            Some(Step::Loaded { id, body }) => {
                docs.insert(
                    id.clone(),
                    LoadedKnowhow {
                        id,
                        body,
                        idle_turns: 0,
                        turns_since_body: 0,
                    },
                );
            }
            Some(Step::Kept { id }) => {
                if let Some(doc) = docs.get_mut(&id) {
                    doc.idle_turns = 0;
                }
            }
            Some(Step::Unloaded { id }) => {
                docs.remove(&id);
            }
            None => {}
        }
    }
    docs.into_values().collect()
}

impl LucidosEngine {
    /// The thread's loaded knowhow right now, read from its events.
    pub(crate) async fn thread_loaded_knowhow(
        &self,
        thread_id: Uuid,
    ) -> Result<Vec<LoadedKnowhow>, Box<dyn std::error::Error + Send + Sync>> {
        let events = self
            .event_store
            .get_thread_knowhow_events(thread_id)
            .await?;
        Ok(loaded_knowhow(&events))
    }
}

#[cfg(test)]
#[path = "loaded_knowhow_tests.rs"]
mod tests;
