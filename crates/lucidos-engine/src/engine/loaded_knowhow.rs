use crate::core::EventRow;
use crate::engine::LucidosEngine;
use crate::llm::tool_names::UNLOAD_KNOWHOW;
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub const IDLE_TURNS_BEFORE_UNLOAD: usize = 3;

use crate::engine::thread_events::TURN_FINISHED_EVENT;
use crate::llm::tool_names::KNOWHOW_TOOLS;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedKnowhow {
    pub id: String,
    pub body: String,
    pub idle_turns: usize,
    turns_since_body: usize,
}

impl LoadedKnowhow {
    /// INVARIANT: Whether this turn's block holds the body: it arrived before a turn
    /// finished. A load in an unfinished turn reads as not held, so the agent
    /// gets the body again rather than none.
    pub fn body_in_block(&self) -> bool {
        self.turns_since_body > 0
    }

    pub fn retention_note(&self) -> String {
        match IDLE_TURNS_BEFORE_UNLOAD.saturating_sub(self.idle_turns) {
            0 => "leaves after this turn unless loaded again".to_string(),
            1 => "stays 1 more turn after this one unless loaded again".to_string(),
            n => format!("stays {n} more turns after this one unless loaded again"),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl LoadedKnowhow {
    pub fn loaded_turns_ago(id: &str, body: &str, idle_turns: usize) -> Self {
        Self {
            id: id.to_string(),
            body: body.to_string(),
            idle_turns,
            turns_since_body: idle_turns,
        }
    }
}

pub fn kept_result(id: &str) -> String {
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

pub fn loaded_knowhow(events: &[EventRow]) -> Vec<LoadedKnowhow> {
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
    pub async fn thread_loaded_knowhow(
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
