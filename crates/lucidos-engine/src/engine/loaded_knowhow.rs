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

#[derive(Default)]
pub struct LoadedKnowhowStore {
    inner: Arc<Mutex<BTreeMap<Uuid, BTreeMap<String, LoadedKnowhow>>>>,
}

impl LoadedKnowhowStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, thread_id: Uuid, doc: LoadedKnowhow) {
        let mut g = self.inner.lock().await;
        g.entry(thread_id).or_default().insert(doc.id.clone(), doc);
    }

    /// Reader consumed by chat::process (engine restart recovery +
    /// user-message injection) and the resume-block builder (which stubs the
    /// body out of resume tool blocks).
    pub async fn for_thread(&self, thread_id: Uuid) -> Vec<LoadedKnowhow> {
        let g = self.inner.lock().await;
        g.get(&thread_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
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
        let mut g = self.inner.lock().await;
        let slot = g.entry(thread_id).or_default();
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
            slot.insert(
                id.to_string(),
                LoadedKnowhow {
                    id: id.to_string(),
                    body: result,
                },
            );
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
        assert_eq!(out.len(), 2, "should recover exactly the two load_knowhow docs");
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
