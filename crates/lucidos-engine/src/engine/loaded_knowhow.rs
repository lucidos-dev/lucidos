//! Per-thread loaded-knowhow store.
//!
//! Tracks knowhow docs loaded by `load_knowhow` for each thread so subsequent
//! turns can dedupe the body out of resume tool blocks and inject a single
//! `[LOADED KNOWHOW]` block into the user message instead. See
//! `docs/plans/2026-05-15-loaded-knowhow-and-context-viewer-reorg.md` Phase 2.
//!
//! Producer: the `load_knowhow` tool handler in `engine/tools/apps.rs`.
//! Consumers: the chat assembly in `engine/chat/process/context_build.rs`
//! (`build_loaded_knowhow_block`) and the resume-block builder in
//! `core/store/messages/resume.rs`. Recovery from `ToolResult` events on
//! engine restart is wired up in [`LoadedKnowhowStore::recover_for_thread`]
//! — the in-memory store is rebuilt by replaying the
//! `(ToolCalled, ToolResult)` pairs for `load_knowhow` for the thread.

use crate::core::EventRow;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct LoadedKnowhow {
    pub id: String,
    // Read by `build_loaded_knowhow_block` in
    // `engine/chat/process/context_build.rs` when assembling the per-turn
    // `[LOADED KNOWHOW]` section, and by the resume-block body-stub swap in
    // `core/store/messages/resume.rs`.
    pub body: String,
}

/// How many threads keep their loaded set in memory. Past it, the thread used
/// longest ago is dropped, and its next turn rebuilds the set from its events.
const MAX_THREADS: usize = 256;

#[derive(Default)]
pub struct LoadedKnowhowStore {
    inner: Arc<Mutex<Slots>>,
}

#[derive(Default)]
struct Slots {
    next_use: u64,
    threads: BTreeMap<Uuid, Slot>,
}

#[derive(Default)]
struct Slot {
    last_use: u64,
    /// Set once the thread's events were replayed into `docs`. A slot that
    /// `insert` recreated after an eviction holds only the docs loaded since.
    replayed: bool,
    docs: BTreeMap<String, LoadedKnowhow>,
}

impl Slots {
    /// The thread's slot, marked as used now. Drops the thread used longest
    /// ago when this one is new and the store is full.
    fn touch(&mut self, thread_id: Uuid) -> &mut Slot {
        if !self.threads.contains_key(&thread_id) && self.threads.len() >= MAX_THREADS {
            let oldest = self
                .threads
                .iter()
                .min_by_key(|(_, slot)| slot.last_use)
                .map(|(id, _)| *id);
            if let Some(oldest) = oldest {
                self.threads.remove(&oldest);
            }
        }
        self.next_use += 1;
        let slot = self.threads.entry(thread_id).or_default();
        slot.last_use = self.next_use;
        slot
    }
}

impl LoadedKnowhowStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, thread_id: Uuid, doc: LoadedKnowhow) {
        let mut g = self.inner.lock().await;
        g.touch(thread_id).docs.insert(doc.id.clone(), doc);
    }

    /// Whether this thread's events still have to be replayed into the store:
    /// after a restart, after an eviction, or before its first replay.
    pub async fn needs_replay(&self, thread_id: Uuid) -> bool {
        let g = self.inner.lock().await;
        !g.threads.get(&thread_id).is_some_and(|slot| slot.replayed)
    }

    /// Reader consumed by chat::process (engine restart recovery +
    /// user-message injection) and the resume-block builder (which stubs the
    /// body out of resume tool blocks).
    pub async fn for_thread(&self, thread_id: Uuid) -> Vec<LoadedKnowhow> {
        let mut g = self.inner.lock().await;
        if !g.threads.contains_key(&thread_id) {
            return Vec::new();
        }
        g.touch(thread_id).docs.values().cloned().collect()
    }

    /// Replay a thread's `(ToolCalled, ToolResult)` pairs for `load_knowhow`
    /// into the in-memory store. The engine MUST be restartable without losing
    /// user-visible state (CLAUDE.md § Engine Statelessness), and the per-
    /// thread loaded set lives only in memory — without recovery, a mid-thread
    /// engine restart would re-inject every previously loaded doc into the
    /// next turn's resume tool blocks (defeating Phase 2's dedupe).
    ///
    /// Idempotent: `BTreeMap` overwrite-on-insert means repeated calls
    /// converge on the same state. Re-uses
    /// [`crate::core::store::collect_tool_pairs_chronological`] so the same
    /// pairing rule (most-recent-pending-by-name with positional fallback)
    /// applies in recovery as in resume-block construction — keeping the two
    /// code paths in lockstep.
    ///
    /// Skips:
    ///   - Pairs with `result.is_none()` — orphaned `ToolCalled` (no result).
    ///   - Pairs whose result body is the canonical "knowhow not found"
    ///     sentinel — only real docs belong in the loaded set, mirroring the
    ///     producer in `engine/tools/apps.rs::load_knowhow_impl`.
    pub async fn recover_for_thread(&self, thread_id: Uuid, events: &[EventRow]) {
        let pairs = crate::core::store::collect_tool_pairs_chronological(events);
        let mut recovered = Vec::new();
        for pair in pairs {
            if pair.tool_name != crate::llm::tool_names::LOAD_KNOWHOW {
                continue;
            }
            let Some(result) = pair.result else { continue };
            if crate::core::knowhow::is_not_found_body(&result) {
                continue;
            }
            let Some(id) = pair
                .args
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            recovered.push(LoadedKnowhow {
                id: id.to_string(),
                body: result,
            });
        }
        // A thread that loaded nothing takes no slot.
        if recovered.is_empty() {
            return;
        }
        let mut g = self.inner.lock().await;
        let slot = g.touch(thread_id);
        slot.replayed = true;
        for doc in recovered {
            slot.docs.insert(doc.id.clone(), doc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn make_event(event_type: &str, payload: serde_json::Value, secs: i64) -> EventRow {
        EventRow {
            id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            payload,
            created: Utc.timestamp_opt(1700000000 + secs, 0).unwrap(),
            thread_id: None,
            sequence: None,
        }
    }

    #[tokio::test]
    async fn insert_and_read_back_for_thread() {
        let store = LoadedKnowhowStore::new();
        let tid = Uuid::new_v4();
        store
            .insert(
                tid,
                LoadedKnowhow {
                    id: "a".into(),
                    body: "AAA".into(),
                },
            )
            .await;
        store
            .insert(
                tid,
                LoadedKnowhow {
                    id: "b".into(),
                    body: "BBB".into(),
                },
            )
            .await;
        let out = store.for_thread(tid).await;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "a"); // BTreeMap preserves id-sorted order
        assert_eq!(out[1].id, "b");
    }

    #[tokio::test]
    async fn dedup_same_id_overwrites() {
        let store = LoadedKnowhowStore::new();
        let tid = Uuid::new_v4();
        store
            .insert(
                tid,
                LoadedKnowhow {
                    id: "a".into(),
                    body: "v1".into(),
                },
            )
            .await;
        store
            .insert(
                tid,
                LoadedKnowhow {
                    id: "a".into(),
                    body: "v2".into(),
                },
            )
            .await;
        let out = store.for_thread(tid).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].body, "v2");
    }

    fn doc(id: &str) -> LoadedKnowhow {
        LoadedKnowhow {
            id: id.into(),
            body: "body".into(),
        }
    }

    /// Every trigger fire is a new thread, so an unbounded store grows for the
    /// engine's whole life. Past the cap the thread used longest ago goes, and
    /// a read counts as a use.
    #[tokio::test]
    async fn the_store_keeps_at_most_max_threads_dropping_the_least_recent() {
        let store = LoadedKnowhowStore::new();
        let threads: Vec<Uuid> = (0..=MAX_THREADS).map(|_| Uuid::new_v4()).collect();
        for &tid in &threads[..MAX_THREADS] {
            store.insert(tid, doc("a")).await;
        }
        // Reading the first thread makes the second the least recent.
        assert_eq!(store.for_thread(threads[0]).await.len(), 1);
        store.insert(threads[MAX_THREADS], doc("a")).await;

        assert_eq!(store.inner.lock().await.threads.len(), MAX_THREADS);
        assert_eq!(store.for_thread(threads[0]).await.len(), 1);
        assert!(store.for_thread(threads[1]).await.is_empty());
        assert_eq!(store.for_thread(threads[MAX_THREADS]).await.len(), 1);
    }

    /// History loads on every turn of every thread, and most load no knowhow.
    #[tokio::test]
    async fn recovering_a_thread_with_no_knowhow_takes_no_slot() {
        let store = LoadedKnowhowStore::new();
        store.recover_for_thread(Uuid::new_v4(), &[]).await;
        assert!(store.inner.lock().await.threads.is_empty());
    }

    #[tokio::test]
    async fn other_threads_isolated() {
        let store = LoadedKnowhowStore::new();
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        store
            .insert(
                t1,
                LoadedKnowhow {
                    id: "a".into(),
                    body: "x".into(),
                },
            )
            .await;
        assert!(store.for_thread(t2).await.is_empty());
    }

    /// An eviction mid-turn, followed by a `load_knowhow`, recreates the slot
    /// with only the new doc. The next turn must still replay the events.
    #[tokio::test]
    async fn a_slot_recreated_after_eviction_still_replays_its_events() {
        let store = LoadedKnowhowStore::new();
        let tid = Uuid::new_v4();
        store.insert(tid, doc("beta")).await;
        assert!(store.needs_replay(tid).await);

        let events = vec![
            make_event(
                "ToolCalled",
                json!({"name": "load_knowhow", "args": {"id": "alpha"}}),
                0,
            ),
            make_event(
                "ToolResult",
                json!({"name": "load_knowhow", "result": "ALPHA BODY", "success": true}),
                1,
            ),
        ];
        store.recover_for_thread(tid, &events).await;

        assert!(!store.needs_replay(tid).await);
        let ids: Vec<String> = store
            .for_thread(tid)
            .await
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(ids, vec!["alpha".to_string(), "beta".to_string()]);
    }

    /// Engine restart loses the in-memory store. Recovery rebuilds it by
    /// replaying each successful `load_knowhow` ToolResult into the slot for
    /// its thread — interleaved tool calls (other tools mixed in) must NOT
    /// shift the load_knowhow pairing.
    #[tokio::test]
    async fn recovery_replays_load_knowhow_results_into_store() {
        let store = LoadedKnowhowStore::new();
        let tid = Uuid::new_v4();
        let events = vec![
            make_event(
                "ToolCalled",
                json!({"name": "load_knowhow", "args": {"id": "alpha"}}),
                0,
            ),
            make_event(
                "ToolResult",
                json!({"name": "load_knowhow", "result": "ALPHA BODY", "success": true}),
                1,
            ),
            // Interleave a different tool — must not pair with load_knowhow.
            make_event(
                "ToolCalled",
                json!({"name": "search_memory", "args": {"q": "x"}}),
                2,
            ),
            make_event(
                "ToolResult",
                json!({"name": "search_memory", "result": "no hits", "success": true}),
                3,
            ),
            make_event(
                "ToolCalled",
                json!({"name": "load_knowhow", "args": {"id": "beta"}}),
                4,
            ),
            make_event(
                "ToolResult",
                json!({"name": "load_knowhow", "result": "BETA BODY", "success": true}),
                5,
            ),
        ];

        store.recover_for_thread(tid, &events).await;
        let out = store.for_thread(tid).await;
        assert_eq!(
            out.len(),
            2,
            "should recover exactly the two load_knowhow docs"
        );
        // BTreeMap preserves id-sorted order: alpha then beta.
        assert_eq!(out[0].id, "alpha");
        assert_eq!(out[0].body, "ALPHA BODY");
        assert_eq!(out[1].id, "beta");
        assert_eq!(out[1].body, "BETA BODY");
    }

    /// Mirror the producer's policy in `engine/tools/apps.rs::load_knowhow_impl`:
    /// not-found responses MUST NOT enter the loaded set on recovery either,
    /// otherwise restart would start dedupling sentinel bodies it never
    /// originally tracked.
    #[tokio::test]
    async fn recovery_skips_not_found_results() {
        let store = LoadedKnowhowStore::new();
        let tid = Uuid::new_v4();
        let events = vec![
            make_event(
                "ToolCalled",
                json!({"name": "load_knowhow", "args": {"id": "real-doc"}}),
                0,
            ),
            make_event(
                "ToolResult",
                json!({"name": "load_knowhow", "result": "[KNOW-HOW: real-doc]\nbody", "success": true}),
                1,
            ),
            make_event(
                "ToolCalled",
                json!({"name": "load_knowhow", "args": {"id": "ghost"}}),
                2,
            ),
            make_event(
                "ToolResult",
                json!({
                    "name": "load_knowhow",
                    "result": crate::core::knowhow::knowhow_not_found_body("ghost"),
                    "success": true,
                }),
                3,
            ),
        ];

        store.recover_for_thread(tid, &events).await;
        let out = store.for_thread(tid).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "real-doc");
    }

    /// Recovery is called once per cold-start, but the store is shared across
    /// threads — defensive idempotency guards against any future caller that
    /// re-runs recovery on the same events.
    #[tokio::test]
    async fn recovery_is_idempotent() {
        let store = LoadedKnowhowStore::new();
        let tid = Uuid::new_v4();
        let events = vec![
            make_event(
                "ToolCalled",
                json!({"name": "load_knowhow", "args": {"id": "alpha"}}),
                0,
            ),
            make_event(
                "ToolResult",
                json!({"name": "load_knowhow", "result": "ALPHA BODY", "success": true}),
                1,
            ),
        ];

        store.recover_for_thread(tid, &events).await;
        store.recover_for_thread(tid, &events).await;
        let out = store.for_thread(tid).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "alpha");
        assert_eq!(out[0].body, "ALPHA BODY");
    }
}
