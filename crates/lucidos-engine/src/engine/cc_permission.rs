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

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// User-facing reason on a denied permission. Surfaces in the response body
/// returned to CC's MCP middleware and in the persisted `Resolved` event.
pub const DENIAL_REASON: &str = "User denied";

/// Reason returned to CC's MCP middleware when `permission_prompt` auto-
/// allows a request via a session-allow match. Echoed in the HTTP response
/// body so CC's tool-call log records *why* the prompt was bypassed; no
/// `CodingAgentPermissionRequest`/`Resolved` event is emitted on the
/// auto-allow path, so this string never reaches the chat UI.
pub const SESSION_ALLOW_REASON: &str = "Allowed for this thread";

/// Grouping key for collapsing identical concurrent permission requests.
/// `canonical_input` is `serde_json::to_string(&input)` — sufficient for CC's
/// repeated identical calls because CC re-serializes the same struct each
/// time, producing the same byte sequence.
pub type DedupKey = (Uuid, String, String);

/// Canonical pending entry — owns the broadcast channel that fans out the
/// answer to every blocked HTTP handler waiting on this `(thread, tool,
/// input)` triple. `tool_name` and `input` are kept on the entry (in addition
/// to being part of the `DedupKey`) so the consent handler can derive an
/// "Always allow" persistence pattern without re-parsing the canonical input.
pub struct CcPermissionEntry {
    pub thread_id: Uuid,
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub tx: tokio::sync::broadcast::Sender<bool>,
}

/// Two-way index: lookup by `DedupKey` when a new HTTP request arrives,
/// lookup by `request_id` when the user submits consent. `session_allows`
/// remembers the user's "Allow for this thread" choices so subsequent
/// identical-pattern requests skip the prompt entirely. In-memory only —
/// engine restart wipes it (sessions resume but the user re-approves once,
/// matching the engine-statelessness rule).
#[derive(Default)]
pub struct CcPermissionState {
    pub by_dedup_key: HashMap<DedupKey, CcPermissionEntry>,
    pub by_request_id: HashMap<String, DedupKey>,
    pub session_allows: HashMap<Uuid, HashSet<String>>,
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

    /// Record a session-allow pattern for a thread. Idempotent — duplicate
    /// inserts are a no-op. The pattern is whatever `derive_allow_pattern`
    /// returned for `AllowScope::Session` on the originating prompt; matching
    /// is exact-string against the set returned by the same derivation on
    /// future prompts.
    pub fn allow_session(&mut self, thread_id: Uuid, pattern: String) {
        self.session_allows
            .entry(thread_id)
            .or_default()
            .insert(pattern);
    }

    /// True iff `pattern` is recorded for `thread_id`. Caller derives the
    /// pattern from the new request's input via `derive_allow_pattern`.
    pub fn matches_session_allow(&self, thread_id: Uuid, pattern: &str) -> bool {
        self.session_allows
            .get(&thread_id)
            .is_some_and(|set| set.contains(pattern))
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
            tool_name: "Edit".to_string(),
            input: serde_json::json!({}),
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
    fn allow_session_inserts_and_matches_per_thread() {
        let mut state = CcPermissionState::default();
        let thread_a = Uuid::new_v4();
        let thread_b = Uuid::new_v4();
        state.allow_session(thread_a, "Edit(/tmp/foo.md)".into());
        assert!(state.matches_session_allow(thread_a, "Edit(/tmp/foo.md)"));
        // Different thread does NOT inherit the allow.
        assert!(!state.matches_session_allow(thread_b, "Edit(/tmp/foo.md)"));
        // Different pattern in the same thread does NOT match.
        assert!(!state.matches_session_allow(thread_a, "Edit(/tmp/bar.md)"));
    }

    #[test]
    fn allow_session_is_idempotent() {
        let mut state = CcPermissionState::default();
        let thread = Uuid::new_v4();
        state.allow_session(thread, "Bash(git:*)".into());
        state.allow_session(thread, "Bash(git:*)".into());
        assert_eq!(
            state.session_allows.get(&thread).map(|s| s.len()),
            Some(1),
            "duplicate insert must not grow the set"
        );
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
