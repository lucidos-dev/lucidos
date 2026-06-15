//! Dedup state for permission prompts, shared by the two permission lanes:
//! the coding agent (Claude Code, via the MCP permission-prompt tool) and the
//! Lucidos Agent's command guard (chat bash/python, ADR 0002). The generic
//! [`PermissionState`] / [`PermissionEntry`] mechanism lives here; the CC-
//! specific superseded/recovery helpers and reason constants are below, and
//! the command-lane specifics live in `engine::command_permission`.
//!
//! Why dedup: a single agent turn can fire several requests for the same
//! logical action (CC's parallel tool_use blocks or sequential retries after a
//! denial). Without dedup, each one would render its own `PermissionCard` —
//! the "infinite loop of file-access prompts" the user saw.
//!
//! This state collapses identical `(thread_id, tool_name, input)` requests
//! onto a single canonical entry: the first request emits the event, every
//! subsequent identical request just subscribes to the same broadcast
//! channel. When the user clicks once, every blocked waiter receives the same
//! answer and the agent continues all of its parallel calls.
//!
//! Lives entirely in memory; on engine restart, in-flight waiters are dropped
//! (CC's MCP client returns deny; a chat command waiter dies with its turn),
//! so there's nothing to recover beyond the orphan-resolution sweeps.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

/// User-facing reason on a denied permission. Surfaces in the response body
/// returned to CC's MCP middleware and in the persisted `Resolved` event.
pub const DENIAL_REASON: &str = "User denied";

/// Reason stamped on a `CodingAgentPermissionResolved` that the engine emits
/// because the user typed a new message instead of answering the permission
/// card. Distinct from `DENIAL_REASON` (an explicit Deny click) and from the
/// orphan-recovery reason in `recover_orphan_cc_permission_requests` (the
/// Claude Code subprocess died first).
pub const SUPERSEDED_REASON: &str = "Superseded by a new message";

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
/// answer to every blocked waiter on this `(thread, tool, input)` triple.
/// `tool_name` and `input` are kept on the entry (in addition to being part
/// of the `DedupKey`) so the consent handler can derive an "Always allow"
/// persistence pattern without re-parsing the canonical input.
pub struct PermissionEntry {
    pub thread_id: Uuid,
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub tx: tokio::sync::broadcast::Sender<bool>,
}

/// Two-way index: lookup by `DedupKey` when a new request arrives, lookup by
/// `request_id` when the user submits consent. `session_allows` remembers the
/// user's "Allow for this thread" choices so subsequent identical-pattern
/// requests skip the prompt entirely. In-memory only — engine restart wipes
/// it (sessions resume but the user re-approves once, matching the
/// engine-statelessness rule).
#[derive(Default)]
pub struct PermissionState {
    pub by_dedup_key: HashMap<DedupKey, PermissionEntry>,
    pub by_request_id: HashMap<String, DedupKey>,
    pub session_allows: HashMap<Uuid, HashSet<String>>,
}

impl PermissionState {
    /// Resolve and remove the entry for `request_id`, returning the broadcast
    /// sender so the caller can fan the answer out to all listeners. Returns
    /// `None` if no permission entry matches (the CC consent endpoint then
    /// falls back to the legacy `pending_mcp_consent` map).
    pub fn take(&mut self, request_id: &str) -> Option<PermissionEntry> {
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

    /// Look up `dedup_key`. If a canonical entry already exists (a duplicate
    /// concurrent request), subscribe to its broadcast and reuse its
    /// `request_id`. Otherwise create a fresh entry, register both indexes, and
    /// return a fresh receiver.
    ///
    /// Returns `(request_id, receiver, is_canonical)`. The caller emits the
    /// `*PermissionRequest(ed)` event only when `is_canonical` is true, so a
    /// burst of identical concurrent requests renders a single card and one
    /// click (fanned out over the broadcast) answers them all. Shared by both
    /// permission lanes — CC's MCP `permission_prompt` handler and the command
    /// guard's in-process block.
    pub fn register_or_attach(
        &mut self,
        dedup_key: DedupKey,
        thread_id: Uuid,
        tool_name: String,
        input: serde_json::Value,
    ) -> (String, tokio::sync::broadcast::Receiver<bool>, bool) {
        // Opportunistic sweep: each new prompt is a chance to evict orphans
        // whose waiters were canceled (CC died, the chat turn was canceled)
        // and would otherwise leak until engine restart.
        self.gc_dead_entries();
        if let Some(entry) = self.by_dedup_key.get(&dedup_key) {
            return (entry.request_id.clone(), entry.tx.subscribe(), false);
        }
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        self.by_dedup_key.insert(
            dedup_key.clone(),
            PermissionEntry {
                thread_id,
                request_id: request_id.clone(),
                tool_name,
                input,
                tx,
            },
        );
        self.by_request_id.insert(request_id.clone(), dedup_key);
        (request_id, rx, true)
    }

    /// Drop entries whose subscribers have all been canceled (waiter futures
    /// dropped because CC died or a chat turn was canceled). With no live
    /// receiver the entry can never deliver an answer; it would otherwise sit
    /// in memory until engine restart. Cheap O(N) sweep — N is bounded by the
    /// user's pending in-flight prompts.
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

/// Outcome of one blocking permission prompt — what the caller relays back
/// to its agent (CC's MCP middleware as JSON; the Codex app-server driver as
/// an approval decision).
pub struct PermissionPromptOutcome {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Render the PermissionCard's one-line summary from the tool call shape.
/// Picks the first recognizable argument; falls back to the bare tool name.
/// `reason` / `grant_root` are last-resort keys for the Codex `file_change`
/// approval, whose input carries no path — without them the card would read
/// as a bare "file_change" with nothing telling the user WHAT is being
/// approved.
pub fn build_permission_summary(tool_name: &str, input: &serde_json::Value) -> String {
    let arg = [
        "file_path",
        "path",
        "command",
        "notebook_path",
        "skill",
        "url",
        "pattern",
        "reason",
        "grant_root",
    ]
    .iter()
    .find_map(|k| input.get(k).and_then(|v| v.as_str()))
    .unwrap_or("");
    let display_name = match tool_name {
        "Skill" => "skill",
        _ => tool_name,
    };
    if arg.is_empty() {
        display_name.to_string()
    } else {
        format!("{} {}", display_name, arg)
    }
}

/// Recover the originating user actor for a coding-agent thread by reading
/// the most recent user-message event on the thread (`MessageReceived` for
/// chat-spawned sessions, `CodingAgentUserMessageSent` for in-session
/// follow-ups). Narrowing to these two event types ensures a later
/// engine-stamped event (e.g. `CodingAgentSettingsChanged`) doesn't overwrite
/// the human actor. Returns `None` if no such event carries an actor —
/// caller falls back to `EventMeta::NONE`.
pub async fn lookup_thread_actor(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<MessageOrigin> {
    let row: Result<Option<(serde_json::Value,)>, sqlx::Error> = sqlx::query_as(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('MessageReceived', 'CodingAgentUserMessageSent') \
           AND payload ? 'actor' \
           AND payload->'actor' != 'null'::jsonb \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((v,))) => serde_json::from_value::<MessageOrigin>(v).ok(),
        Ok(None) => None,
        Err(e) => {
            crate::log!(
                "[CCPermission] lookup_thread_actor failed for thread {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// One blocking permission round-trip — the shared core both permission
/// raise paths drive:
///
///   * CC's MCP HTTP path (`api::internal::permission_prompt`, invoked by
///     `lucidos mcp-permission-server`)
///   * the Codex app-server bridge (the `permission_rx` select arm in
///     `run_session/run.rs`, fed by `item/*/requestApproval` JSON-RPC
///     requests)
///
/// Flow: session-allow pre-check (an earlier "Allow for this thread" click
/// whose pattern matches skips the prompt entirely) → dedup
/// `register_or_attach` (identical concurrent requests share one card) → if
/// canonical, emit `CodingAgentPermissionRequest` (rendered as a
/// PermissionCard) → wait **indefinitely** on the broadcast for the user's
/// click. The paired `CodingAgentPermissionResolved` is emitted by the
/// consent endpoint (`api::mcp::submit_mcp_consent`) so it fires once per
/// click, not once per deduped listener.
pub async fn prompt_coding_agent_permission(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    tool_use_id: String,
    tool_name: String,
    input: serde_json::Value,
) -> PermissionPromptOutcome {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};

    let session_pattern = derive_allow_pattern(&tool_name, &input, AllowScope::Session);
    let is_session_allowed = match session_pattern.as_deref() {
        Some(p) => {
            let pending = pending.lock().unwrap();
            pending.matches_session_allow(thread_id, p)
        }
        None => false,
    };
    if is_session_allowed {
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(SESSION_ALLOW_REASON.to_string()),
        };
    }

    let canonical_input = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    let dedup_key: DedupKey = (thread_id, tool_name.clone(), canonical_input);
    let summary = build_permission_summary(&tool_name, &input);

    let (request_id, mut rx, is_canonical) = {
        let mut pending = pending.lock().unwrap();
        pending.register_or_attach(dedup_key, thread_id, tool_name.clone(), input.clone())
    };

    if is_canonical {
        // Neither raise path carries a header-borne actor (an MCP subprocess
        // / a JSON-RPC request from the agent child). Recover the
        // originating actor from the thread's last user message.
        let actor = lookup_thread_actor(pool, thread_id).await;
        let meta = match actor {
            Some(a) => EventMeta::with_actor(Some(a)),
            None => EventMeta::NONE,
        };
        event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentPermissionRequest {
                        request_id: request_id.clone(),
                        tool_use_id,
                        tool_name,
                        input,
                        summary,
                    },
                    meta,
                },
                "[CCPermission] CodingAgentPermissionRequest",
            )
            .await;
    }

    // Wait forever for the user — the user is the rate-limiter. A closed
    // channel (entry swept / engine teardown) reads as deny.
    let allowed = rx.recv().await.unwrap_or(false);
    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };
    PermissionPromptOutcome { allowed, reason }
}

/// Resolve every unresolved `CodingAgentPermissionRequest` on `thread_id` as
/// denied, because the user typed a new message instead of clicking a button
/// on the permission card. Two effects, mirroring
/// `recover_orphan_cc_permission_requests` but scoped to one live thread:
///
///   1. Fan a `false` (deny) out to any still-blocked MCP handler via the
///      in-memory broadcast entry, so the Claude Code subprocess's pending
///      `tools/call` returns immediately instead of dangling until the next
///      `gc_dead_entries` sweep.
///   2. Emit `CodingAgentPermissionResolved { allowed: false }` so the
///      PermissionCard's buttons stop dangling — without this the card sits
///      clickable forever (clicking it 404s once CC interrupts and the waiter
///      is gc'd) and the thread reads as stuck on `waiting_for_user_answer`.
///      The projection flips the thread status back to `running`.
///
/// No-op when nothing is pending / everything is already resolved. The caller
/// still routes the typed message to CC as a normal follow-up — this only
/// clears the stale card.
pub async fn resolve_pending_permissions_as_superseded(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    let rows: Vec<(Option<String>,)> = match sqlx::query_as(
        "SELECT e.payload->>'request_id' \
         FROM events e \
         WHERE e.event_type = 'CodingAgentPermissionRequest' \
           AND e.thread_id = $1 \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'CodingAgentPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log!(
                "[CCPermission] unresolved-permission query failed for {}: {}",
                thread_id,
                e
            );
            return;
        }
    };

    for (request_id,) in rows {
        let Some(request_id) = request_id.filter(|s| !s.is_empty()) else {
            continue;
        };
        // Best-effort unblock of a still-waiting MCP handler. The std Mutex is
        // released before the `.await` below — never hold it across an await.
        {
            let mut state = pending.lock().unwrap();
            if let Some(entry) = state.take(&request_id) {
                let _ = entry.tx.send(false);
            }
        }
        event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentPermissionResolved {
                        request_id,
                        allowed: false,
                        reason: Some(SUPERSEDED_REASON.to_string()),
                        persist_scope: None,
                    },
                    meta: EventMeta::with_actor(actor.clone()),
                },
                "[CCPermission] CodingAgentPermissionResolved (superseded)",
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_permission_summary_uses_file_path() {
        let s = build_permission_summary(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md", "old_string": "x" }),
        );
        assert_eq!(s, "Edit /tmp/foo.md");
    }

    #[test]
    fn build_permission_summary_falls_back_to_command() {
        let s = build_permission_summary("Bash", &serde_json::json!({ "command": "ls -la" }));
        assert_eq!(s, "Bash ls -la");
        // The Codex app-server bridge raises command approvals under the
        // backend's own tool vocabulary — the summary must surface the
        // command the same way.
        let s = build_permission_summary(
            "command_execution",
            &serde_json::json!({ "command": "sudo rm -rf /tmp/x", "cwd": "/wt" }),
        );
        assert_eq!(s, "command_execution sudo rm -rf /tmp/x");
    }

    #[test]
    fn build_permission_summary_returns_tool_name_when_no_arg_field() {
        let s = build_permission_summary("WeirdTool", &serde_json::json!({ "foo": 1 }));
        assert_eq!(s, "WeirdTool");
    }

    #[test]
    fn build_permission_summary_uses_skill_for_skill_tool() {
        let s = build_permission_summary("Skill", &serde_json::json!({ "skill": "update-config" }));
        assert_eq!(s, "skill update-config");
    }

    #[test]
    fn build_permission_summary_uses_url_for_webfetch() {
        let s = build_permission_summary(
            "WebFetch",
            &serde_json::json!({ "url": "https://example.com", "prompt": "x" }),
        );
        assert_eq!(s, "WebFetch https://example.com");
    }

    fn register(
        state: &mut PermissionState,
        key: DedupKey,
    ) -> (String, tokio::sync::broadcast::Receiver<bool>, bool) {
        let tool_name = key.1.clone();
        state.register_or_attach(key, Uuid::nil(), tool_name, serde_json::json!({}))
    }

    #[test]
    fn register_or_attach_creates_canonical_entry_first_time() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (request_id, _rx, is_canonical) = register(&mut state, key.clone());
        assert!(is_canonical, "first request must be canonical");
        assert!(state.by_dedup_key.contains_key(&key));
        assert!(state.by_request_id.contains_key(&request_id));
    }

    #[test]
    fn register_or_attach_returns_existing_request_id_for_duplicate() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (first_id, _rx1, first_canonical) = register(&mut state, key.clone());
        let (second_id, _rx2, second_canonical) = register(&mut state, key.clone());
        assert!(first_canonical);
        assert!(!second_canonical, "duplicate must not be canonical");
        assert_eq!(
            first_id, second_id,
            "duplicate must reuse the canonical request_id"
        );
    }

    #[test]
    fn register_or_attach_stores_tool_name_and_input_on_canonical_entry() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Skill".into(), "{\"skill\":\"x:y\"}".into());
        let (request_id, _rx, _) = state.register_or_attach(
            key,
            Uuid::nil(),
            "Skill".into(),
            serde_json::json!({"skill": "x:y"}),
        );
        let entry = state.take(&request_id).unwrap();
        assert_eq!(entry.tool_name, "Skill");
        assert_eq!(entry.input, serde_json::json!({"skill": "x:y"}));
    }

    #[tokio::test]
    async fn duplicate_subscribers_both_receive_the_answer() {
        let mut state = PermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (id, mut rx1, _) = register(&mut state, key.clone());
        let (_, mut rx2, _) = register(&mut state, key.clone());

        // Resolve via the same path the consent endpoint uses.
        let entry = state.take(&id).expect("entry must be present");
        let _ = entry.tx.send(true);

        assert!(rx1.recv().await.unwrap());
        assert!(rx2.recv().await.unwrap());
    }

    fn entry(req_id: &str) -> PermissionEntry {
        let (tx, _rx) = tokio::sync::broadcast::channel(1);
        PermissionEntry {
            thread_id: Uuid::nil(),
            request_id: req_id.to_string(),
            tool_name: "Edit".to_string(),
            input: serde_json::json!({}),
            tx,
        }
    }

    #[test]
    fn take_returns_none_for_unknown_request_id() {
        let mut state = PermissionState::default();
        assert!(state.take("missing").is_none());
    }

    #[test]
    fn take_removes_from_both_indexes() {
        let mut state = PermissionState::default();
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
        let mut state = PermissionState::default();
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
        let mut state = PermissionState::default();
        let thread = Uuid::new_v4();
        state.allow_session(thread, "Bash(git:*)".into());
        state.allow_session(thread, "Bash(git:*)".into());
        assert_eq!(
            state.session_allows.get(&thread).map(|s| s.len()),
            Some(1),
            "duplicate insert must not grow the set"
        );
    }

    /// Seed the thread as a coding-agent thread — the lifecycle guard
    /// rejects `CodingAgentPermissionRequest` on a thread it classifies as
    /// Chat, which is exactly what an unseeded test thread looks like.
    async fn seed_cc_thread(bus: &crate::engine::event_bus::EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                coding_agent: crate::runtime::CodingAgent::Codex,
                session_id: "sid-test".into(),
                branch: "claude-code/test".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
            },
            meta: EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("SessionStarted emit")
        .expect("SessionStarted persisted");
    }

    /// The full blocking round-trip both raise paths (CC MCP HTTP, Codex
    /// app-server bridge) share: the canonical caller emits ONE
    /// `CodingAgentPermissionRequest`, the user's click (the consent
    /// endpoint's `take` + broadcast) resolves every deduped waiter, and the
    /// outcome surfaces `allowed` correctly.
    #[tokio::test]
    async fn prompt_round_trip_emits_request_and_resolves_on_broadcast() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let waiter = {
            let pool = pool.clone();
            let bus = bus.clone();
            let pending = pending.clone();
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    thread_id,
                    "i1".to_string(),
                    "command_execution".to_string(),
                    serde_json::json!({"command": "sudo ls"}),
                )
                .await
            })
        };

        // Wait for the canonical entry to register, then resolve it the way
        // the consent endpoint does.
        let request_id = loop {
            let id = {
                let state = pending.lock().unwrap();
                state.by_request_id.keys().next().cloned()
            };
            if let Some(id) = id {
                break id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        let entry = {
            let mut state = pending.lock().unwrap();
            state.take(&request_id).expect("canonical entry present")
        };
        assert_eq!(entry.tool_name, "command_execution");
        let _ = entry.tx.send(true);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("resolves within 10s")
            .expect("task ok");
        assert!(outcome.allowed);
        assert!(outcome.reason.is_none());

        // The persisted card: exactly one request event, carrying the
        // backend-shaped tool name and the item id as tool_use_id.
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_all(&pool)
        .await
        .expect("query events");
        assert_eq!(rows.len(), 1, "one card per canonical request");
        assert_eq!(rows[0].0["tool_name"], "command_execution");
        assert_eq!(rows[0].0["tool_use_id"], "i1");
        assert_eq!(rows[0].0["summary"], "command_execution sudo ls");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A prior "Allow for this thread" click must short-circuit the prompt:
    /// no event, immediate allow with the session-allow reason.
    #[tokio::test]
    async fn prompt_skips_card_on_session_allow_match() {
        use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();

        let input = serde_json::json!({"command": "git status"});
        let pattern = derive_allow_pattern("Bash", &input, AllowScope::Session)
            .expect("bash derives a session pattern");
        pending.lock().unwrap().allow_session(thread_id, pattern);

        let outcome = prompt_coding_agent_permission(
            &pool,
            &bus,
            &pending,
            thread_id,
            "i2".to_string(),
            "Bash".to_string(),
            input,
        )
        .await;
        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(SESSION_ALLOW_REASON));

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(count, 0, "session-allow must not render a card");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn gc_dead_entries_removes_orphans_with_no_receivers() {
        let mut state = PermissionState::default();
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
