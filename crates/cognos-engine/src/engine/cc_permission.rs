//! Dedup state for CC permission prompts.
//!
//! CC can fire several `tools/call` for the same logical action in a single
//! assistant turn (parallel tool_use blocks, or sequential retries after a
//! denial). Without dedup, each one would render its own `PermissionCard` —
//! the "infinite loop of file-access prompts" the user sees.
//!
//! This state collapses identical `(thread_id, tool_name, input)` requests
//! onto a single canonical entry: the first request emits the event, every
//! subsequent identical request just subscribes to the same broadcast
//! channel. When the user clicks once, every blocked HTTP handler receives
//! the same answer and CC continues all of its parallel calls.
//!
//! Lives entirely in memory; on engine restart, in-flight CC HTTP calls are
//! dropped (their MCP client returns deny), so there's nothing to recover.

use std::collections::HashMap;
use uuid::Uuid;

/// User-facing reason on a denied permission. Surfaces in the response body
/// returned to CC's MCP middleware and in the persisted `Resolved` event.
pub const DENIAL_REASON: &str = "User denied";

/// Grouping key for collapsing identical concurrent permission requests.
/// `canonical_input` is `serde_json::to_string(&input)` — sufficient for CC's
/// repeated identical calls because CC re-serializes the same struct each
/// time, producing the same byte sequence.
pub type DedupKey = (Uuid, String, String);

/// Canonical pending entry — owns the broadcast channel that fans out the
/// answer to every blocked HTTP handler waiting on this `(thread, tool,
/// input)` triple.
pub struct CcPermissionEntry {
    pub thread_id: Uuid,
    pub request_id: String,
    pub tx: tokio::sync::broadcast::Sender<bool>,
}

/// Two-way index: lookup by `DedupKey` when a new HTTP request arrives,
/// lookup by `request_id` when the user submits consent.
#[derive(Default)]
pub struct CcPermissionState {
    pub by_dedup_key: HashMap<DedupKey, CcPermissionEntry>,
    pub by_request_id: HashMap<String, DedupKey>,
}

impl CcPermissionState {
    /// Resolve and remove the entry for `request_id`, returning the broadcast
    /// sender so the caller can fan the answer out to all listeners. Returns
    /// `None` if no CC permission entry matches (the consent endpoint then
    /// falls back to the legacy `pending_mcp_consent` map).
    pub fn take(&mut self, request_id: &str) -> Option<CcPermissionEntry> {
        let key = self.by_request_id.remove(request_id)?;
        self.by_dedup_key.remove(&key)
    }

    /// Drop entries whose subscribers have all been canceled (HTTP handler
    /// futures dropped because CC died or the MCP request was aborted). With
    /// no live receiver the entry can never deliver an answer; it would
    /// otherwise sit in memory until engine restart. Cheap O(N) sweep — N is
    /// bounded by the user's pending in-flight prompts.
    pub fn gc_dead_entries(&mut self) {
        let dead: Vec<DedupKey> = self
            .by_dedup_key
            .iter()
            .filter(|(_, entry)| entry.tx.receiver_count() == 0)
            .map(|(k, _)| k.clone())
            .collect();
        for key in dead {
            if let Some(entry) = self.by_dedup_key.remove(&key) {
                self.by_request_id.remove(&entry.request_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(req_id: &str) -> CcPermissionEntry {
        let (tx, _rx) = tokio::sync::broadcast::channel(1);
        CcPermissionEntry {
            thread_id: Uuid::nil(),
            request_id: req_id.to_string(),
            tx,
        }
    }

    #[test]
    fn take_returns_none_for_unknown_request_id() {
        let mut state = CcPermissionState::default();
        assert!(state.take("missing").is_none());
    }

    #[test]
    fn take_removes_from_both_indexes() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".to_string(), "{}".to_string());
        state.by_dedup_key.insert(key.clone(), entry("req-1"));
        state.by_request_id.insert("req-1".to_string(), key.clone());

        let taken = state.take("req-1").expect("entry must be returned");
        assert_eq!(taken.request_id, "req-1");
        assert!(state.by_dedup_key.is_empty());
        assert!(state.by_request_id.is_empty());
    }

    #[test]
    fn gc_dead_entries_removes_orphans_with_no_receivers() {
        let mut state = CcPermissionState::default();
        // A live entry — keep its receiver alive on the stack.
        let live_key: DedupKey = (Uuid::nil(), "Edit".into(), "{\"a\":1}".into());
        let live_entry = entry("req-live");
        let _live_rx = live_entry.tx.subscribe();
        state.by_dedup_key.insert(live_key.clone(), live_entry);
        state
            .by_request_id
            .insert("req-live".into(), live_key.clone());

        // A dead entry — no subscriber.
        let dead_key: DedupKey = (Uuid::nil(), "Edit".into(), "{\"b\":2}".into());
        state.by_dedup_key.insert(dead_key.clone(), entry("req-dead"));
        state
            .by_request_id
            .insert("req-dead".into(), dead_key.clone());

        state.gc_dead_entries();

        assert!(state.by_dedup_key.contains_key(&live_key), "live kept");
        assert!(state.by_request_id.contains_key("req-live"), "live kept");
        assert!(!state.by_dedup_key.contains_key(&dead_key), "dead swept");
        assert!(!state.by_request_id.contains_key("req-dead"), "dead swept");
    }
}
