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
//! In-flight waiters live entirely in memory; on engine restart they are
//! dropped (CC's MCP client returns deny; a chat command waiter dies with its
//! turn), so there's nothing to recover beyond the orphan-resolution sweeps.
//!
//! The per-thread session-allow set is different: it is a **cache** over
//! durable state. The grants are persisted as
//! `CodingAgentPermissionResolved { allowed: true, persist_scope: "session" }`,
//! and [`hydrate_session_allows`] refills a thread's set from the event store
//! on the first prompt after a restart — otherwise an Apply-with-restart would
//! silently drop grants the user had already clicked while the thread itself
//! resumed.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use uuid::Uuid;

use crate::engine::command_guard::{
    self, unwrap_shell_command, RiskLane, SideEffectCategory, StaticVerdict,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventMeta, MessageOrigin, ThreadDirection, ThreadEvent,
};
use crate::llm::tool_names as tn;
use crate::triggers::TriggerConfig;

/// User-facing reason on a denied permission. Surfaces in the response body
/// returned to CC's MCP middleware and in the persisted `Resolved` event.
pub const DENIAL_REASON: &str = "User denied";

/// Reason returned to the coding agent when a pending permission prompt resolves
/// because the broadcast channel CLOSED — i.e. the engine is tearing down
/// (restart), NOT because the user clicked Deny. Distinct from `DENIAL_REASON`:
/// a restart is not a user decision, so a resumed session must not read it as
/// "the user rejected my approach". The companion half is the
/// restart-not-rejection note carried by the recovery system prompts (see
/// `RESTART_NOT_REJECTION_RULE` in `agent_session::prompts`).
pub const RESTART_INTERRUPT_REASON: &str =
    "Interrupted by an engine restart — not a user decision. Re-attempt the action; \
     the restart did not reject your approach.";

/// Reason stamped on a `CodingAgentPermissionResolved` that the engine emits
/// because the user typed a new message instead of answering the permission
/// card. Distinct from `DENIAL_REASON` (an explicit Deny click) and from the
/// orphan-recovery reason in `recover_orphan_cc_permission_requests` (the
/// Claude Code subprocess died first).
pub const SUPERSEDED_REASON: &str = "Superseded by a new message";

/// Reason stamped on a `CodingAgentPermissionResolved` that the engine emits
/// when a coding-agent session IDLES with a permission card still unresolved
/// (e.g. a workflow whose parallel subagent's card dangled while the main turn
/// finished). Cleared at the turn boundary in `emit_coding_agent_idled` so the
/// stale card can't sit clickable and a later click can't resurrect the
/// finished thread. Distinct from `SUPERSEDED_REASON` (the user typed instead
/// of clicking) and from the boot orphan-recovery reason (engine restart).
pub const SESSION_ENDED_REASON: &str =
    "Coding agent session ended before answering — request expired";

/// Reason returned to CC's MCP middleware when `permission_prompt` auto-
/// allows a request via a session-allow match. Echoed in the HTTP response
/// body so CC's tool-call log records *why* the prompt was bypassed; no
/// `CodingAgentPermissionRequest`/`Resolved` event is emitted on the
/// auto-allow path, so this string never reaches the chat UI.
pub const SESSION_ALLOW_REASON: &str = "Allowed for this thread";

/// Reason returned when the request is a file write landing inside the
/// session's own worktree (see [`worktree_write_auto_allowed`]). Like the
/// session-allow and unattended fast paths, this emits NO
/// `CodingAgentPermissionRequest`/`Resolved` event, so the string never reaches
/// the chat UI — it only rides the response body so the agent's tool log
/// records *why* the prompt was bypassed.
pub const WORKTREE_WRITE_ALLOW_REASON: &str =
    "Auto-allowed: file write inside this session's own worktree";

/// Reason on an unattended auto-ALLOW of a benign in-workspace request (a read,
/// an in-workspace write/edit, git, `lucidos data write`, …). The coding-agent
/// session was launched by a trigger with no human to answer a card, so the
/// engine resolves immediately. Like the session-allow fast path, the
/// unattended path emits NO `CodingAgentPermissionRequest`/`Resolved` event, so
/// this string never reaches the chat UI — it only rides the response body so
/// the agent's tool log records *why* the prompt was bypassed.
pub const UNATTENDED_ALLOW_BENIGN_REASON: &str =
    "Auto-allowed: benign in-workspace operation (unattended trigger session)";

/// Reason on an unattended auto-ALLOW of an irreversible side-effect whose
/// category is covered by the originating trigger's side-effect grant.
pub const UNATTENDED_ALLOW_GRANTED_REASON: &str =
    "Auto-allowed: covered by the trigger's side-effect grant";

/// Reason on an unattended auto-DENY of a catastrophic command — denied
/// regardless of any grant (the command guard's hard deny-list).
pub const UNATTENDED_DENY_CATASTROPHIC_REASON: &str =
    "Auto-denied: catastrophic operation, never permitted unattended";

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
/// requests skip the prompt entirely.
///
/// `session_allows` is a **cache, not the source of truth** — the grants are
/// durable in the event store (`CodingAgentPermissionResolved` with
/// `persist_scope: "session"`), and [`hydrate_session_allows`] refills a
/// thread's set from there on the first prompt after an engine restart. Before
/// that existed, an Apply-with-restart silently dropped every grant the user
/// had clicked while the thread itself resumed, so the same file re-asked
/// minutes later.
///
/// `hydrated_threads` is that cache's per-thread "already refilled" marker. It
/// is set only after a SUCCESSFUL read, so a transient DB failure retries on
/// the next prompt instead of pinning an empty set. Only the coding-agent lane
/// hydrates today; the command lane (`Engine::pending_command_permission`)
/// shares this struct but persists a `command` string rather than a tool
/// `input`, so it needs its own extractor before it can adopt the same shape.
#[derive(Default)]
pub struct PermissionState {
    pub by_dedup_key: HashMap<DedupKey, PermissionEntry>,
    pub by_request_id: HashMap<String, DedupKey>,
    pub session_allows: HashMap<Uuid, HashSet<String>>,
    pub hydrated_threads: HashSet<Uuid>,
}

impl PermissionState {
    /// Resolve and remove the entry for `request_id`, returning the broadcast
    /// sender so the caller can fan the answer out to all listeners. Returns
    /// `None` if no permission entry matches (the consent endpoint then 404s).
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
pub async fn lookup_thread_actor(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<MessageOrigin> {
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

/// Refill `thread_id`'s in-memory session-allow set from the event store, once
/// per thread per engine lifetime.
///
/// "Allow for this thread" is durable in the events (a
/// `CodingAgentPermissionResolved` carrying `allowed: true` and
/// `persist_scope: "session"`), but `PermissionState::session_allows` is only a
/// cache — so before this existed, an engine restart between the click and the
/// next matching request re-asked the user. Threads survive a restart and
/// resume; the grant the user gave them must too.
///
/// Deliberately **lazy** rather than a boot sweep: the work lands on the one
/// path that was about to block on a human anyway, costs a single query per
/// thread, and never scans threads that don't prompt.
///
/// Two properties make this safe:
///
///   * **Only genuine session grants hydrate.** The `allowed` + `persist_scope`
///     filter excludes Allow-once (no scope), Deny, the `narrow`/`broad` scopes
///     (which live in `cc-allowed-tools`, not here), and every engine-emitted
///     resolution — supersession, session-ended, and boot orphan-recovery all
///     write `allowed: false`.
///   * **The pattern is re-derived, never re-stored.** `derive_allow_pattern`
///     is the same call `api::mcp::record_allow_grant` used to record the
///     grant, so the two sites cannot drift; a stored string could.
///
/// `r.thread_id = $1` is a **trust boundary, not just a scope filter** — keep
/// it. This query turns persisted rows into standing permission grants, so it
/// must only read rows the engine itself wrote through `BusEvent::Thread`.
/// `POST /api/v1/events/emit` (reachable from an app UI, and from a
/// coding-agent session via `lucidos data emit`) lets a caller choose an
/// arbitrary `event_type`, and its `is_reserved_type_name` guard covers
/// `SystemEvent` names only — `CodingAgentPermissionResolved` is a
/// `ThreadEvent`, so the name itself is not refused. What closes the hole is
/// that a domain event is persisted with a NULL `thread_id`, so a forged row
/// can never satisfy this predicate. Widening the join to match on
/// `request_id` alone would let an agent grant itself a standing allow for any
/// tool.
///
/// That boundary has to hold on BOTH aliases, which is why `q` carries its own
/// `thread_id = $1`. The resolved row `r` being thread-scoped is not enough: the
/// REQUEST row is what supplies `tool_name` and `input` to
/// `derive_allow_pattern`, so a forged request matched on `request_id` alone
/// could pair a legitimate grant with an attacker-chosen tool.
///
/// Fails toward asking: a query error logs and leaves the thread unhydrated, so
/// the card renders and the next prompt retries.
async fn hydrate_session_allows(
    pool: &sqlx::PgPool,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
) {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};

    {
        let state = pending.lock().unwrap();
        if state.hydrated_threads.contains(&thread_id) {
            return;
        }
    } // Lock released before the await — never hold it across one.

    let rows: Vec<(Option<String>, Option<serde_json::Value>)> = match sqlx::query_as(
        "SELECT q.payload->>'tool_name', q.payload->'input' \
         FROM events r \
         JOIN events q \
           ON q.event_type = 'CodingAgentPermissionRequest' \
          AND q.thread_id = $1 \
          AND q.payload->>'request_id' = r.payload->>'request_id' \
         WHERE r.event_type = 'CodingAgentPermissionResolved' \
           AND r.thread_id = $1 \
           AND (r.payload->>'allowed')::boolean IS TRUE \
           AND r.payload->>'persist_scope' = 'session'",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log!(
                "[CCPermission] session-allow hydration failed for thread {}: {} \
                 — the card will render and the next prompt retries",
                thread_id,
                e
            );
            return;
        }
    };

    let patterns: Vec<String> = rows
        .into_iter()
        .filter_map(|(tool_name, input)| {
            derive_allow_pattern(&tool_name?, &input?, AllowScope::Session)
        })
        .collect();

    let mut state = pending.lock().unwrap();
    let count = patterns.len();
    for pattern in patterns {
        state.allow_session(thread_id, pattern);
    }
    // Marked only on the success path, so a failed read above retries.
    state.hydrated_threads.insert(thread_id);
    if count > 0 {
        crate::log!(
            "[CCPermission] rehydrated {} session-allow pattern(s) for thread {}",
            count,
            thread_id
        );
    }
}

/// Whether a coding-agent session has a human reachable to answer a permission
/// card. Decided by walking the spawn tree to its root (see
/// [`resolve_attend_mode`]): a human device at the root ⇒ [`AttendMode::Interactive`]
/// (render a card, wait); a trigger/scheduler (or any engine origin) at the root
/// ⇒ [`AttendMode::Unattended`] (auto-resolve, never hang).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttendMode {
    /// A human is reachable at the root of the spawn tree — render a card and
    /// wait indefinitely (the pre-existing behavior).
    Interactive,
    /// No human is reachable. Auto-resolve permission requests immediately.
    /// `grant` is the originating trigger's side-effect grant (empty when the
    /// root is the engine but not a scheduler-fired trigger).
    Unattended { grant: Vec<SideEffectCategory> },
}

/// Hard cap on the spawn-tree walk so a malformed parent chain (or a cycle) can
/// never loop forever. Real spawn trees are shallow; a chain this deep is
/// almost certainly automated, so the cap-exceeded fallback is `Unattended`.
const MAX_ANCESTRY_HOPS: usize = 16;

/// Read a thread's *originating* `MessageOrigin` — the earliest event that
/// carries one, among the spawn-defining event types. `TriggerStarted` (a direct
/// scheduler-fired thread) and `MessageReceived` (chat or an agent-spawned
/// sub-thread) both stamp a `MessageOrigin` into `payload->'origin'`. Returns
/// `None` when no such event carries an origin (legacy rows / odd threads),
/// which the caller treats as interactive (preserve the pre-existing behavior).
///
/// Also returns that row's `parent_thread_id`, the *callback linkage*, which is
/// a strictly narrower thing than the origin: a `relation: "top"` spawn and a
/// parent's child follow-up both stamp a `ThreadLink` origin with NO linkage.
/// `resolve_attend_mode` walks up only where the linkage exists, so attribution
/// alone never carries a trigger's grant across a boundary the user asked to be
/// independent.
async fn fetch_thread_origin_and_linkage(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<(MessageOrigin, Option<Uuid>)> {
    let row: Result<Option<(serde_json::Value, Option<Uuid>)>, sqlx::Error> = sqlx::query_as(
        "SELECT payload->'origin', (payload->>'parent_thread_id')::uuid FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('TriggerStarted', 'MessageReceived') \
           AND payload ? 'origin' \
           AND payload->'origin' != 'null'::jsonb \
         ORDER BY sequence ASC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((v, parent_thread_id))) => serde_json::from_value::<MessageOrigin>(v)
            .ok()
            .map(|origin| (origin, parent_thread_id)),
        Ok(None) => None,
        Err(e) => {
            crate::log!(
                "[CCPermission] fetch_thread_origin_and_linkage failed for {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// Decide whether this coding-agent session is interactive (a human can answer a
/// card) or unattended, and — when unattended — what side-effect grant it
/// inherits. Walks the spawn tree from `thread_id` up to its root via the
/// persisted `MessageOrigin` chain:
///
///   * `Device` / human-mode `Api`/`Workspace` at the root ⇒ `Interactive`.
///   * `Engine { Scheduler { trigger_id } }` ⇒ `Unattended` with that trigger's
///     `side_effect_grant` (looked up in the in-memory registry, rebuilt from
///     events at boot — so this is restart-safe).
///   * any other engine / system / non-human origin ⇒ `Unattended` with an
///     empty grant.
///   * `ThreadLink { direction: Parent }` **whose event also carries the
///     `parent_thread_id` callback linkage** ⇒ hop to the parent and continue.
///     A `ThreadLink` without linkage is attribution only (a `relation: "top"`
///     spawn, or a parent's child follow-up): it names who launched the thread
///     for the route popover, and deliberately does NOT lend that spawning thread's
///     attend mode or side-effect grant. Unclassifiable ⇒ `Interactive`, so an
///     independent spawn asks a human rather than auto-resolving.
///
/// Everything is derived from already-persisted events plus the trigger
/// registry — no new persistence, no spawn-time plumbing. A user-rooted tree
/// stays interactive even when an agent spawned the leaf coding-agent thread, so
/// a human watching can still answer; only trigger/engine-rooted trees
/// auto-resolve.
pub async fn resolve_attend_mode(
    pool: &sqlx::PgPool,
    trigger_configs: &Arc<RwLock<HashMap<String, TriggerConfig>>>,
    thread_id: Uuid,
) -> AttendMode {
    let mut current = thread_id;
    let mut seen: HashSet<Uuid> = HashSet::new();
    for _ in 0..MAX_ANCESTRY_HOPS {
        if !seen.insert(current) {
            break; // cycle guard — should never happen for a tree
        }
        let Some((origin, callback_linkage)) = fetch_thread_origin_and_linkage(pool, current).await
        else {
            // No recorded origin — preserve the pre-existing interactive behavior
            // rather than auto-resolving a thread we can't classify.
            return AttendMode::Interactive;
        };
        match origin {
            MessageOrigin::Device { .. } => return AttendMode::Interactive,
            MessageOrigin::Api {
                mode,
                source_thread_id,
                ..
            } => match (mode, source_thread_id) {
                (ActorMode::Human, _) => return AttendMode::Interactive,
                (_, Some(parent)) => {
                    current = parent;
                }
                (_, None) => return AttendMode::Unattended { grant: Vec::new() },
            },
            MessageOrigin::Workspace { mode, .. } => {
                return match mode {
                    ActorMode::Human => AttendMode::Interactive,
                    _ => AttendMode::Unattended { grant: Vec::new() },
                };
            }
            MessageOrigin::ThreadLink { direction, .. } => {
                // Only a Parent link means "the linked thread spawned me"; a
                // Child callback shouldn't be a thread's originating origin, but
                // be safe and treat it as non-human.
                if direction != ThreadDirection::Parent {
                    return AttendMode::Unattended { grant: Vec::new() };
                }
                // Hop via the LINKAGE, not the origin's `thread_id`. The two
                // name the same thread on every row the engine writes, but this
                // walk decides privilege, and `parent_thread_id` is the field
                // that owns parent-ness; the origin only owns display. Absent
                // linkage is a `relation: "top"` spawn: it names its spawning thread
                // for the popover but is NOT in that spawning thread's tree, so the
                // walk stops rather than inheriting a trigger's grant.
                match callback_linkage {
                    Some(parent) => current = parent,
                    None => return AttendMode::Interactive,
                }
            }
            MessageOrigin::Engine { reason } => {
                return match reason {
                    EngineReason::Scheduler { trigger_id, .. } => {
                        let grant = trigger_configs
                            .read()
                            .ok()
                            .and_then(|m| m.get(&trigger_id).map(|c| c.side_effect_grant.clone()))
                            .unwrap_or_default();
                        AttendMode::Unattended { grant }
                    }
                    _ => AttendMode::Unattended { grant: Vec::new() },
                };
            }
            MessageOrigin::System => return AttendMode::Unattended { grant: Vec::new() },
        }
    }
    // Depth/cycle exceeded — a chain this deep is automated; never hang.
    AttendMode::Unattended { grant: Vec::new() }
}

/// Static classification of one coding-agent permission request, deciding how an
/// unattended session resolves it. Deterministic — reuses the command guard's
/// static passes, never the LLM judge (the permission path must not be able to
/// stall).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestVerdict {
    /// Benign in-workspace work — allow even with an empty grant.
    Benign,
    /// A catastrophic, irreversible operation — deny regardless of any grant.
    Catastrophic,
    /// An irreversible real-world side-effect of this category — allow only when
    /// the trigger's grant contains it.
    SideEffect(SideEffectCategory),
}

/// Extract the shell command from a coding-agent *command* request. Codex raises
/// these as `command_execution`; Claude Code as `Bash`.
fn coding_agent_command<'a>(tool_name: &str, input: &'a serde_json::Value) -> Option<&'a str> {
    if !matches!(tool_name, "command_execution" | "Bash") {
        return None;
    }
    input.get("command").and_then(|v| v.as_str())
}

/// Extract the target path from a coding-agent *file-write* request. Codex
/// raises these as `file_change` (carrying `grant_root`); Claude Code as
/// `Edit`/`Write`/`MultiEdit`/`NotebookEdit` (carrying `file_path`/`path`).
fn coding_agent_file_target(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    if !matches!(
        tool_name,
        "file_change" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
    ) {
        return None;
    }
    for key in ["file_path", "path", "notebook_path", "grant_root"] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// True when `path` provably targets somewhere OUTSIDE the workspace root. A
/// `..` component (absolute OR relative) can escape and can't be proven
/// contained lexically → treat as outside (conservative: grant-gated). A
/// relative path with no `..` resolves against the worktree (inside the
/// workspace) → inside. Otherwise check containment against the workspace root.
fn path_outside_workspace(path: &str, workspace_path: &Path) -> bool {
    let p = Path::new(path);
    // Checked FIRST, before the relative-is-inside shortcut — a relative
    // `../../etc/...` escapes the worktree just as an absolute one does.
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return true;
    }
    if !p.is_absolute() {
        return false;
    }
    !p.starts_with(workspace_path)
}

/// A path component that disqualifies a target from the in-worktree fast path:
/// `..` (can't be proven contained lexically) or `.git` (git metadata — see
/// [`path_inside_worktree`]).
fn has_rejected_component(p: &Path) -> bool {
    p.components().any(|c| match c {
        std::path::Component::ParentDir => true,
        std::path::Component::Normal(name) => name == ".git",
        _ => false,
    })
}

/// Canonicalize the longest existing prefix of `p`. A `Write` names a file that
/// doesn't exist yet, so canonicalizing the target itself would fail on exactly
/// the case we most need to classify — walk up to the nearest ancestor that
/// does resolve. `None` when nothing along the chain resolves.
fn canonical_existing_prefix(p: &Path) -> Option<std::path::PathBuf> {
    let mut current = Some(p);
    while let Some(candidate) = current {
        if let Ok(real) = std::fs::canonicalize(candidate) {
            return Some(real);
        }
        current = candidate.parent();
    }
    None
}

/// True when `path` provably resolves INSIDE `worktree_root` — the session's own
/// disposable worktree — and is therefore covered by the reviewed-before-Apply
/// guarantee that makes [`worktree_write_auto_allowed`] safe.
///
/// "Provably" is load-bearing: a false positive here skips a security card, so
/// every branch that can't *prove* containment returns false.
///
///   * **Symlinks are resolved, not trusted lexically.** A lexical prefix check
///     is escapable — a symlink inside the worktree pointing at an external
///     directory makes `<worktree>/link/.claude/settings.json` look contained
///     while the write lands outside, invisible to the reviewed diff. So both
///     sides are canonicalized: the root, and the longest existing prefix of
///     the target (see [`canonical_existing_prefix`]). Canonicalizing the
///     target also catches the case where the target file is itself a symlink
///     out of the tree. If either side fails to resolve, containment is
///     unproven → false.
///   * **A `..` component** (checked lexically before resolution) → false.
///   * **A `.git` component** → false, checked BOTH on the input path and on
///     the resolved worktree-relative path, so a symlink pointing into git
///     metadata is caught too. Writes there (a `.git/hooks/pre-commit` that
///     runs on the worktree's next commit) do not appear in the diff the user
///     reviews before Apply, so they don't inherit the justification for
///     auto-allowing everything else in here.
///   * **A relative path** → false. Resolving one needs the agent's cwd, which
///     is the worktree for a repo-rooted spawn but `data/apps/<id>` beneath it
///     for an app coding-agent thread — so the worktree root alone can't
///     resolve it. Costs nothing in practice: Claude Code's file tools require
///     an absolute `file_path` and Codex's `grant_root` is absolute, and the
///     fallback is just the card that rendered before this fast path existed.
fn path_inside_worktree(path: &str, worktree_root: &Path) -> bool {
    let p = Path::new(path);
    if !p.is_absolute() || has_rejected_component(p) {
        return false;
    }
    let Ok(root) = std::fs::canonicalize(worktree_root) else {
        return false;
    };
    let Some(resolved) = canonical_existing_prefix(p) else {
        return false;
    };
    let Ok(relative) = resolved.strip_prefix(&root) else {
        return false;
    };
    !has_rejected_component(relative)
}

/// True when this coding-agent permission request is a file write landing
/// inside the session's own worktree, and can therefore be auto-allowed without
/// rendering a card.
///
/// Why this exists: Claude Code auto-approves in-cwd writes under
/// `--permission-mode acceptEdits` **except** under `.claude/` and `.git/`,
/// which it routes through `--permission-prompt-tool` in every mode and
/// regardless of `--allowedTools` (see `CC_PROTECTED_PATH_MARKERS` in
/// `engine::claude_code`). Lucidos keeps all of its own agent configuration in
/// `.claude/`, so editing a rule or a skill cost a card on every single edit,
/// and the persisted "Always allow" scopes can't suppress it. The engine's
/// policy is stated as the simpler invariant — *an in-worktree file write needs
/// no card* — whose only observable delta is exactly CC's protected paths.
///
/// It is safe because the worktree is disposable and every change in it is
/// reviewed in the Diff before Apply. `.git` is carved out of
/// [`path_inside_worktree`] precisely because it is the one in-worktree
/// location that ISN'T in that diff.
///
/// Scope is the file-write vocabulary of [`coding_agent_file_target`] —
/// commands (`Bash` / `command_execution`) are deliberately NOT covered: a
/// command can do anything, so it stays on the card path even when it merely
/// mentions a `.claude/` file. `None` for `worktree_root` (no live session
/// entry, or the session never recorded one) fails closed.
pub fn worktree_write_auto_allowed(
    tool_name: &str,
    input: &serde_json::Value,
    worktree_root: Option<&Path>,
) -> bool {
    let Some(root) = worktree_root else {
        return false;
    };
    coding_agent_file_target(tool_name, input)
        .is_some_and(|target| path_inside_worktree(&target, root))
}

/// Classify one coding-agent permission request for the unattended decision.
///
/// * Command requests (`command_execution` / `Bash`): reuse the command guard's
///   STATIC classification by normalizing onto its bash tool vocabulary — the
///   catastrophic deny-list, the obviously-safe allowlist, and the static
///   side-effect/destruction fallback all apply, with no LLM judge. An
///   in-workspace destruction (`ReversibleDanger`) counts as benign here (it's
///   recoverable and in-workspace); an irreversible side-effect carries its
///   category for the grant check.
/// * File requests (`file_change` / `Edit` / `Write` / …): an in-workspace
///   target is benign; a target outside the workspace root is out-of-workspace
///   destruction (grant-gated).
/// * Anything else (reads, etc.) is benign.
pub fn classify_coding_agent_request(
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
) -> RequestVerdict {
    if let Some(cmd) = coding_agent_command(tool_name, input) {
        // Codex wraps commands as `/bin/zsh -lc '<script>'`; classify the inner
        // script so a wrapped side-effect isn't hidden behind the `zsh` head.
        let synthetic = serde_json::json!({ "command": unwrap_shell_command(cmd) });
        return match command_guard::static_classify(tn::RUN_BASH, &synthetic) {
            StaticVerdict::Settled(RiskLane::Catastrophic) => RequestVerdict::Catastrophic,
            // `static_classify` only ever settles Safe/Catastrophic; map the
            // rest defensively to benign.
            StaticVerdict::Settled(_) => RequestVerdict::Benign,
            StaticVerdict::NeedsJudge(ji) => {
                let judged = command_guard::fallback_classify(&ji);
                match judged.lane {
                    RiskLane::Catastrophic => RequestVerdict::Catastrophic,
                    RiskLane::IrreversibleDanger => RequestVerdict::SideEffect(
                        judged.category.unwrap_or(SideEffectCategory::Other),
                    ),
                    RiskLane::Safe | RiskLane::ReversibleDanger => RequestVerdict::Benign,
                }
            }
        };
    }
    if let Some(path) = coding_agent_file_target(tool_name, input) {
        if path_outside_workspace(&path, workspace_path) {
            return RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction);
        }
        return RequestVerdict::Benign;
    }
    RequestVerdict::Benign
}

/// Resolve a permission request for an unattended session from its classified
/// verdict and the inherited grant. Returns `(allowed, reason)`.
fn decide_unattended(verdict: RequestVerdict, grant: &[SideEffectCategory]) -> (bool, String) {
    match verdict {
        RequestVerdict::Benign => (true, UNATTENDED_ALLOW_BENIGN_REASON.to_string()),
        RequestVerdict::Catastrophic => (false, UNATTENDED_DENY_CATASTROPHIC_REASON.to_string()),
        RequestVerdict::SideEffect(cat) => {
            if grant.contains(&cat) {
                (true, UNATTENDED_ALLOW_GRANTED_REASON.to_string())
            } else {
                (
                    false,
                    format!(
                        "Auto-denied: this coding-agent session runs unattended under a trigger \
                         that did not grant {} — add the matching category to the trigger's \
                         side-effect grant to allow it.",
                        cat.reason()
                    ),
                )
            }
        }
    }
}

/// The per-request payload a backend funnels into
/// [`prompt_coding_agent_permission`] — the call-specific data, kept distinct
/// from the engine-infra handles (`pool` / `event_bus` / `pending` /
/// `trigger_configs` / `workspace_path`) the chokepoint also needs.
pub struct CodingAgentPermissionInput {
    pub thread_id: Uuid,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

/// Read the worktree root of `thread_id`'s live agent session, if any.
///
/// For the **out-of-process** raise path (CC's MCP permission server POSTing to
/// `api::internal::permission_prompt`) the registry is the only way in — the
/// request carries a thread id and a tool call, nothing about the worktree. The
/// **in-process** Codex bridge instead reads `run_session`'s own
/// `worktree_path` local, which is the value that seeded this registry entry,
/// so both paths see the same root without the bridge taking a lock in the
/// engine's highest-traffic loop.
///
/// `None` — no live session entry, or a session that never recorded a worktree
/// — makes [`worktree_write_auto_allowed`] fail closed.
pub async fn lookup_session_worktree(
    agent_sessions: &tokio::sync::Mutex<HashMap<Uuid, crate::engine::types::AgentSession>>,
    thread_id: Uuid,
) -> Option<std::path::PathBuf> {
    let sessions = agent_sessions.lock().await;
    sessions.get(&thread_id)?.worktree_path.clone()
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
/// Flow: **in-worktree write fast path** (a file write landing inside the
/// session's own worktree needs no card — see [`worktree_write_auto_allowed`];
/// checked first because it is pure, lock-free and DB-free) → session-allow
/// pre-check (an earlier "Allow for this thread" click whose pattern matches
/// skips the prompt entirely, rehydrated from persisted events so it survives
/// an engine restart) → **unattended fast path**
/// (a trigger/engine-rooted session has no human to answer a card, so
/// [`resolve_attend_mode`] + [`classify_coding_agent_request`] +
/// [`decide_unattended`] resolve it immediately — benign in-workspace work
/// auto-allows, an irreversible side-effect auto-allows iff the originating
/// trigger granted its category, everything else auto-denies; emits no card
/// events, so the session never hangs) → otherwise (interactive) dedup
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
    trigger_configs: &Arc<RwLock<HashMap<String, TriggerConfig>>>,
    workspace_path: &Path,
    worktree_path: Option<&Path>,
    request: CodingAgentPermissionInput,
) -> PermissionPromptOutcome {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};

    let CodingAgentPermissionInput {
        thread_id,
        tool_use_id,
        tool_name,
        input,
    } = request;

    // Cheapest gate first: a pure path check, no lock and no DB.
    if worktree_write_auto_allowed(&tool_name, &input, worktree_path) {
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(WORKTREE_WRITE_ALLOW_REASON.to_string()),
        };
    }

    // Rebuild this thread's "Allow for this thread" grants from persisted
    // events if we haven't yet in this engine lifetime, so a restart between
    // the click and the next matching request doesn't re-ask.
    hydrate_session_allows(pool, pending, thread_id).await;

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

    // Unattended (trigger/engine-rooted) sessions have no human to answer a
    // card — resolve immediately from the originating trigger's inherited
    // side-effect grant plus a static benign check, so the session NEVER hangs.
    // Like the session-allow fast path above, this emits no card events. An
    // auto-allow surfaces as the normal CodingAgentToolCalled/Result; an
    // auto-deny surfaces in the agent's own failure report.
    if let AttendMode::Unattended { grant } =
        resolve_attend_mode(pool, trigger_configs, thread_id).await
    {
        let verdict = classify_coding_agent_request(&tool_name, &input, workspace_path);
        let (allowed, reason) = decide_unattended(verdict, &grant);
        crate::log!(
            "[CCPermission] unattended auto-resolve thread={} tool={} -> {} ({})",
            thread_id,
            tool_name,
            if allowed { "allow" } else { "deny" },
            reason
        );
        return PermissionPromptOutcome {
            allowed,
            reason: Some(reason),
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

    // Wait forever for the user — the user is the rate-limiter. The decision is
    // factored into `outcome_from_permission_recv` so the three-way split is
    // unit-testable without a DB.
    outcome_from_permission_recv(rx.recv().await)
}

/// Map the broadcast `recv` result for a pending permission into the outcome
/// relayed to the agent. The three outcomes are distinct:
///   * `Ok(true)`  — an explicit Allow fanned over the broadcast.
///   * `Ok(false)` — an explicit Deny (incl. supersession's `send(false)`).
///   * `Err(_)`    — the channel CLOSED. This can only mean the engine is tearing
///     down (restart): every live resolution path `send`s before dropping the
///     sender, and `gc_dead_entries` never reaps an entry whose receiver is still
///     awaiting. A restart is NOT a user denial, so it carries the neutral
///     `RESTART_INTERRUPT_REASON` — otherwise a resumed session reads "User
///     denied" in its transcript and treats the restart as the user rejecting its
///     approach.
fn outcome_from_permission_recv(
    recv: Result<bool, tokio::sync::broadcast::error::RecvError>,
) -> PermissionPromptOutcome {
    match recv {
        Ok(true) => PermissionPromptOutcome {
            allowed: true,
            reason: None,
        },
        Ok(false) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(DENIAL_REASON.to_string()),
        },
        Err(_) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(RESTART_INTERRUPT_REASON.to_string()),
        },
    }
}

/// Resolve every unresolved `CodingAgentPermissionRequest` on `thread_id` as
/// denied — because the user typed a new message instead of clicking a button
/// on the permission card. Thin wrapper over
/// [`resolve_pending_permissions_with_reason`]; the caller still routes the
/// typed message to CC as a normal follow-up, this only clears the stale card.
pub async fn resolve_pending_permissions_as_superseded(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    resolve_pending_permissions_with_reason(
        pool,
        event_bus,
        pending,
        thread_id,
        actor,
        SUPERSEDED_REASON,
        "[CCPermission] CodingAgentPermissionResolved (superseded)",
    )
    .await;
}

/// Resolve every unresolved `CodingAgentPermissionRequest` on `thread_id` as
/// denied — because the coding-agent session IDLED with a card still dangling
/// (a workflow whose parallel subagent's card outlived the main turn, a
/// canceled turn, etc.). Thin wrapper over
/// [`resolve_pending_permissions_with_reason`]. Called from
/// `emit_coding_agent_idled` at the turn boundary: by then the session/turn is
/// done, so any still-pending card is orphaned and clearing it is safe (a live
/// card blocks the turn, so idle cannot fire during a genuine wait).
pub async fn resolve_pending_permissions_as_session_ended(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    resolve_pending_permissions_with_reason(
        pool,
        event_bus,
        pending,
        thread_id,
        actor,
        SESSION_ENDED_REASON,
        "[CCPermission] CodingAgentPermissionResolved (session ended)",
    )
    .await;
}

/// Shared core of the "clear every unresolved `CodingAgentPermissionRequest` on
/// this thread as denied" sweeps — the superseded path (user typed instead of
/// clicking) and the session-ended path (the session idled with a card
/// dangling). Mirrors `recover_orphan_cc_permission_requests` but scoped to one
/// thread. Two effects per unresolved request:
///
///   1. Fan a `false` (deny) out to any still-blocked MCP handler via the
///      in-memory broadcast entry, so the Claude Code subprocess's pending
///      `tools/call` returns immediately instead of dangling until the next
///      `gc_dead_entries` sweep.
///   2. Emit `CodingAgentPermissionResolved { allowed: false }` so the
///      PermissionCard's buttons stop dangling — without this the card sits
///      clickable forever (clicking it 404s once CC interrupts and the waiter
///      is gc'd) and the thread reads as stuck.
///
/// A resolution now flips the thread to `running` ONLY from
/// `waiting_for_user_answer` (see `event_bus_projection_thread.rs`), so emitting
/// these on an already-idle thread clears the stale card WITHOUT resurrecting it
/// into a dead `running`. `reason` / `log_label` distinguish the two callers in
/// the persisted event and the logs. No-op when nothing is pending.
async fn resolve_pending_permissions_with_reason(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
    reason: &str,
    log_label: &str,
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
                        reason: Some(reason.to_string()),
                        persist_scope: None,
                    },
                    meta: EventMeta::with_actor(actor.clone()),
                },
                log_label,
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

    #[test]
    fn outcome_allow_has_no_reason() {
        let o = outcome_from_permission_recv(Ok(true));
        assert!(o.allowed);
        assert_eq!(o.reason, None);
    }

    #[test]
    fn outcome_explicit_deny_is_user_denied() {
        // A `false` fanned over the broadcast (explicit Deny click or
        // supersession) keeps the "User denied" reason.
        let o = outcome_from_permission_recv(Ok(false));
        assert!(!o.allowed);
        assert_eq!(o.reason.as_deref(), Some(DENIAL_REASON));
    }

    #[test]
    fn outcome_closed_channel_is_restart_not_user_denied() {
        // A closed channel means the engine tore down (restart). It must NOT
        // surface as "User denied" — a resumed session would read that as the
        // user rejecting its approach.
        use tokio::sync::broadcast::error::RecvError;
        let o = outcome_from_permission_recv(Err(RecvError::Closed));
        assert!(!o.allowed);
        assert_eq!(o.reason.as_deref(), Some(RESTART_INTERRUPT_REASON));
        assert_ne!(o.reason.as_deref(), Some(DENIAL_REASON));
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

    fn empty_trigger_configs() -> Arc<RwLock<HashMap<String, TriggerConfig>>> {
        Arc::new(RwLock::new(HashMap::new()))
    }

    /// Build a one-entry trigger registry whose trigger carries `grant`.
    fn trigger_configs_with(
        trigger_id: &str,
        grant: Vec<SideEffectCategory>,
    ) -> Arc<RwLock<HashMap<String, TriggerConfig>>> {
        let payload = serde_json::json!({
            "trigger_id": trigger_id,
            "name": "Test Trigger",
            "schedule": ["0 0 3 * * *"],
            "timezone": "UTC",
            "run": { "type": "intent", "intent": "do nightly work" },
            "side_effect_grant": serde_json::to_value(&grant).unwrap(),
        });
        let config = TriggerConfig::from_created_payload(&payload).expect("build TriggerConfig");
        let mut map = HashMap::new();
        map.insert(trigger_id.to_string(), config);
        Arc::new(RwLock::new(map))
    }

    /// Insert a raw event row carrying `{"origin": <origin>}` so the resolver's
    /// `payload->'origin'` query has something to read — cheaper than driving a
    /// full MessageReceived through the bus + lifecycle guard.
    ///
    /// Origin only, no `parent_thread_id`: that is the shape of a thread with no
    /// callback linkage (a `relation: "top"` spawn, a trigger, a device chat).
    /// A child spawn carries both, so it uses [`insert_child_spawn_event`].
    async fn insert_origin_event(
        pool: &sqlx::PgPool,
        thread_id: Uuid,
        event_type: &str,
        origin: &MessageOrigin,
    ) {
        let payload = serde_json::json!({ "origin": origin });
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
             VALUES ($1, 'thread', $2, $3, $4, NOW(), $2::uuid)",
        )
        .bind(Uuid::new_v4())
        .bind(thread_id.to_string())
        .bind(event_type)
        .bind(payload)
        .execute(pool)
        .await
        .expect("insert origin event");
    }

    /// A `relation: "child"` spawn's `MessageReceived`: the `ThreadLink` origin
    /// AND the `parent_thread_id` callback linkage, exactly as
    /// `make_message_received` writes it. The linkage is what licenses the
    /// resolver's hop, so a fixture that omitted it would silently stop testing
    /// the walk.
    async fn insert_child_spawn_event(pool: &sqlx::PgPool, thread_id: Uuid, parent: Uuid) {
        let payload = serde_json::json!({
            "origin": parent_link(parent),
            "parent_thread_id": parent,
        });
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
             VALUES ($1, 'thread', $2, 'MessageReceived', $3, NOW(), $2::uuid)",
        )
        .bind(Uuid::new_v4())
        .bind(thread_id.to_string())
        .bind(payload)
        .execute(pool)
        .await
        .expect("insert child spawn event");
    }

    fn parent_link(parent: Uuid) -> MessageOrigin {
        MessageOrigin::ThreadLink {
            thread_id: parent,
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        }
    }

    fn scheduler_origin(trigger_id: &str) -> MessageOrigin {
        MessageOrigin::engine(EngineReason::Scheduler {
            trigger_id: trigger_id.to_string(),
            trigger_name: None,
        })
    }

    // --- classifier (pure, no DB) ------------------------------------------

    #[test]
    fn classify_benign_in_workspace_command_allows() {
        // `lucidos data write` is not a recognized side-effect / destruction
        // shape, so it classifies benign — the reported work-tracker write.
        let v = classify_coding_agent_request(
            "command_execution",
            &serde_json::json!({
                "command": "lucidos data write artifacts/work-tracker/data.json --from /tmp/x.json"
            }),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::Benign);
    }

    #[test]
    fn classify_external_api_command_is_side_effect() {
        let v = classify_coding_agent_request(
            "command_execution",
            &serde_json::json!({"command": "curl -X POST https://example.com/api -d @data"}),
            Path::new("/ws"),
        );
        assert_eq!(
            v,
            RequestVerdict::SideEffect(SideEffectCategory::ExternalApi)
        );
    }

    #[test]
    fn classify_email_command_is_side_effect() {
        let v = classify_coding_agent_request(
            "Bash",
            &serde_json::json!({"command": "echo body | mail -s hi a@b.com"}),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::SideEffect(SideEffectCategory::Email));
    }

    #[test]
    fn classify_catastrophic_command_denied() {
        let v = classify_coding_agent_request(
            "command_execution",
            &serde_json::json!({"command": "rm -rf /"}),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::Catastrophic);
    }

    #[test]
    fn classify_in_workspace_file_write_is_benign() {
        let v = classify_coding_agent_request(
            "file_change",
            &serde_json::json!({"grant_root": "/ws/data/artifacts"}),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::Benign);
    }

    #[test]
    fn classify_out_of_workspace_file_write_is_side_effect() {
        let v = classify_coding_agent_request(
            "Write",
            &serde_json::json!({"file_path": "/etc/cron.d/evil"}),
            Path::new("/ws"),
        );
        assert_eq!(
            v,
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction)
        );
    }

    #[test]
    fn classify_relative_dotdot_file_write_is_side_effect() {
        // A relative target that escapes the worktree via `..` must NOT slip
        // through as benign — it's grant-gated out-of-workspace destruction.
        let v = classify_coding_agent_request(
            "Edit",
            &serde_json::json!({"file_path": "../../etc/cron.d/evil"}),
            Path::new("/ws"),
        );
        assert_eq!(
            v,
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction)
        );
    }

    #[test]
    fn classify_unknown_tool_is_benign() {
        let v = classify_coding_agent_request(
            "Read",
            &serde_json::json!({"file_path": "/etc/passwd"}),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::Benign);
    }

    #[test]
    fn unwrap_shell_command_cases() {
        // The realistic Codex shape.
        assert_eq!(
            unwrap_shell_command("/bin/zsh -lc 'curl -X POST https://x -d @y'"),
            "curl -X POST https://x -d @y"
        );
        assert_eq!(unwrap_shell_command("bash -c \"rm -rf /\""), "rm -rf /");
        // `sh -c <script> [$0 [arg ...]]`: operands after the script only set $0
        // and the positional params, so the script is still what runs. Cutting
        // at the matching close quote (not just unwrapping when the quotes wrap
        // the whole remainder) is what keeps this from classifying as `'rm`.
        assert_eq!(
            unwrap_shell_command("bash -c 'rm -rf /' ignored"),
            "rm -rf /"
        );
        assert_eq!(
            unwrap_shell_command("/bin/zsh -lc 'curl -X POST https://x' zsh arg1"),
            "curl -X POST https://x"
        );
        // Unterminated quote: no matching close, so it is returned as-is rather
        // than guessed at.
        assert_eq!(unwrap_shell_command("bash -c 'rm -rf"), "'rm -rf");
        // Not a shell wrapper → unchanged (Claude Code's raw Bash command).
        assert_eq!(
            unwrap_shell_command("curl -X POST https://x"),
            "curl -X POST https://x"
        );
        // Shell with no -c flag → unchanged.
        assert_eq!(unwrap_shell_command("zsh script.sh"), "zsh script.sh");
    }

    #[test]
    fn classify_zsh_wrapped_side_effect_is_detected() {
        // Codex wraps everything in `/bin/zsh -lc '…'`; the inner side-effect
        // must still be seen (otherwise the grant check is bypassed).
        let v = classify_coding_agent_request(
            "command_execution",
            &serde_json::json!({"command": "/bin/zsh -lc 'curl -X POST https://example.com -d @x'"}),
            Path::new("/ws"),
        );
        assert_eq!(
            v,
            RequestVerdict::SideEffect(SideEffectCategory::ExternalApi)
        );
    }

    #[test]
    fn classify_zsh_wrapped_benign_is_benign() {
        let v = classify_coding_agent_request(
            "command_execution",
            &serde_json::json!({
                "command": "/bin/zsh -lc 'lucidos data write artifacts/work-tracker/data.json --from /tmp/x.json'"
            }),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::Benign);
    }

    #[test]
    fn classify_zsh_wrapped_catastrophic_is_catastrophic() {
        let v = classify_coding_agent_request(
            "command_execution",
            &serde_json::json!({"command": "/bin/zsh -lc 'rm -rf /'"}),
            Path::new("/ws"),
        );
        assert_eq!(v, RequestVerdict::Catastrophic);
    }

    // --- in-worktree write fast path (pure, but real paths on disk) ---------

    /// A worktree-shaped fixture: `<tmp>/wt` holding `.claude/rules/`, `.git/`,
    /// a `vendor/dep/.git/` nested repo, plus a sibling `<tmp>/outside` and a
    /// symlink `<tmp>/wt/escape` pointing at it. Containment is resolved
    /// against the real filesystem, so these have to exist.
    struct WorktreeFixture {
        _tmp: tempfile::TempDir,
        root: std::path::PathBuf,
        outside: std::path::PathBuf,
    }

    fn worktree_fixture() -> WorktreeFixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("wt");
        let outside = tmp.path().join("outside");
        for dir in [
            root.join(".claude/rules"),
            root.join(".git/hooks"),
            root.join("vendor/dep/.git"),
            root.join("crates/lucidos-engine/src"),
            outside.join(".claude"),
        ] {
            std::fs::create_dir_all(&dir).expect("create fixture dir");
        }
        std::fs::write(root.join(".gitignore"), "target\n").expect("write .gitignore");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");
        WorktreeFixture {
            _tmp: tmp,
            root,
            outside,
        }
    }

    /// Render an absolute path under the fixture root as a `&str`-able String.
    fn under(root: &Path, rel: &str) -> String {
        root.join(rel).to_string_lossy().into_owned()
    }

    #[test]
    fn path_inside_worktree_accepts_targets_under_the_root() {
        let f = worktree_fixture();
        assert!(path_inside_worktree(
            &under(&f.root, ".claude/rules/frontend.md"),
            &f.root
        ));
        // A file that doesn't exist yet (a `Write`) — resolution walks up to
        // the nearest existing ancestor.
        assert!(path_inside_worktree(
            &under(&f.root, "crates/lucidos-engine/src/brand_new.rs"),
            &f.root
        ));
        // A directory that doesn't exist yet either.
        assert!(path_inside_worktree(
            &under(&f.root, ".claude/skills/run-tests/SKILL.md"),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_targets_outside_the_root() {
        let f = worktree_fixture();
        // The real out-of-worktree case from the event stream: the user's
        // global CC config. Must keep rendering a card.
        assert!(!path_inside_worktree(
            &under(&f.outside, ".claude/settings.json"),
            &f.root
        ));
        assert!(!path_inside_worktree("/etc/cron.d/evil", &f.root));
    }

    /// The escape a purely lexical prefix check would have allowed: a symlink
    /// inside the worktree pointing at an external directory. The write lands
    /// outside and never shows up in the reviewed diff, so it must still ask.
    #[cfg(unix)]
    #[test]
    fn path_inside_worktree_rejects_symlink_escapes() {
        let f = worktree_fixture();
        // Through the symlinked directory, into an existing external dir…
        assert!(!path_inside_worktree(
            &under(&f.root, "escape/.claude/settings.json"),
            &f.root
        ));
        // …and to a file that doesn't exist yet beyond it (the `Write` case,
        // where resolution has to walk up through the symlink).
        assert!(!path_inside_worktree(
            &under(&f.root, "escape/.claude/brand_new.json"),
            &f.root
        ));
    }

    /// A symlink pointing INTO the worktree's own git metadata must be caught
    /// by the post-resolution `.git` check, not just the lexical one.
    #[cfg(unix)]
    #[test]
    fn path_inside_worktree_rejects_symlink_into_git_metadata() {
        let f = worktree_fixture();
        std::os::unix::fs::symlink(f.root.join(".git"), f.root.join("gitlink"))
            .expect("symlink into .git");
        assert!(!path_inside_worktree(
            &under(&f.root, "gitlink/hooks/pre-commit"),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_prefix_sibling_of_the_root() {
        let f = worktree_fixture();
        let sibling = f.root.with_file_name("wt-sibling");
        std::fs::create_dir_all(sibling.join("src")).expect("create sibling");
        assert!(!path_inside_worktree(
            &sibling.join("src/main.rs").to_string_lossy(),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_parent_dir_escapes() {
        let f = worktree_fixture();
        assert!(!path_inside_worktree("../../etc/cron.d/evil", &f.root));
        assert!(!path_inside_worktree(
            &under(&f.root, "../outside/.claude/settings.json"),
            &f.root
        ));
    }

    #[test]
    fn path_inside_worktree_rejects_relative_paths() {
        // Resolving a relative path needs the agent's cwd, which differs
        // between repo-rooted and app coding-agent threads — so it fails
        // closed rather than guessing the worktree root.
        let f = worktree_fixture();
        assert!(!path_inside_worktree(".claude/rules/frontend.md", &f.root));
    }

    #[test]
    fn path_inside_worktree_rejects_git_metadata_at_any_depth() {
        // Git metadata is the one in-worktree location that does NOT show up
        // in the diff the user reviews before Apply, so it keeps its card.
        let f = worktree_fixture();
        assert!(!path_inside_worktree(
            &under(&f.root, ".git/hooks/pre-commit"),
            &f.root
        ));
        assert!(!path_inside_worktree(
            &under(&f.root, "vendor/dep/.git/config"),
            &f.root
        ));
        // A file merely NAMED like it is fine — the check is component-wise.
        assert!(path_inside_worktree(&under(&f.root, ".gitignore"), &f.root));
    }

    #[test]
    fn path_inside_worktree_fails_closed_on_an_unresolvable_root() {
        // A worktree that has been removed (stale session entry) can't prove
        // containment of anything.
        let f = worktree_fixture();
        let gone = f.root.join("never-existed");
        assert!(!path_inside_worktree(
            &under(&f.root, ".claude/rules/frontend.md"),
            &gone
        ));
    }

    #[test]
    fn worktree_write_auto_allowed_covers_the_file_write_tools() {
        let f = worktree_fixture();
        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            let key = if tool == "NotebookEdit" {
                "notebook_path"
            } else {
                "file_path"
            };
            let input = serde_json::json!({ key: under(&f.root, ".claude/rules/db.md") });
            assert!(
                worktree_write_auto_allowed(tool, &input, Some(&f.root)),
                "{tool} on an in-worktree .claude/ path must skip the card"
            );
        }
    }

    #[test]
    fn worktree_write_auto_allowed_ignores_commands() {
        // A command can do anything — it stays on the card path even when it
        // only mentions a `.claude/` file.
        let f = worktree_fixture();
        for tool in ["Bash", "command_execution"] {
            let input = serde_json::json!({
                "command": format!("rm -rf {}", under(&f.root, ".claude/rules"))
            });
            assert!(
                !worktree_write_auto_allowed(tool, &input, Some(&f.root)),
                "{tool} must never take the worktree fast path"
            );
        }
    }

    #[test]
    fn worktree_write_auto_allowed_fails_closed_without_a_known_worktree() {
        let input = serde_json::json!({ "file_path": "/anything/x.md" });
        assert!(
            !worktree_write_auto_allowed("Edit", &input, None),
            "an unknown worktree root must render the card"
        );
    }

    #[test]
    fn worktree_write_auto_allowed_rejects_out_of_worktree_and_git_targets() {
        let f = worktree_fixture();
        assert!(!worktree_write_auto_allowed(
            "Edit",
            &serde_json::json!({ "file_path": under(&f.outside, ".claude/settings.json") }),
            Some(&f.root)
        ));
        assert!(!worktree_write_auto_allowed(
            "Write",
            &serde_json::json!({ "file_path": under(&f.root, ".git/hooks/pre-commit") }),
            Some(&f.root)
        ));
    }

    #[test]
    fn path_outside_workspace_cases() {
        let ws = Path::new("/ws");
        assert!(!path_outside_workspace("/ws/data/x", ws));
        assert!(!path_outside_workspace("relative/path", ws)); // relative, no .. → inside
        assert!(path_outside_workspace("/etc/passwd", ws));
        assert!(path_outside_workspace("/ws/../etc", ws)); // absolute .. → outside
                                                           // Relative `..` escapes the worktree too, so it must be caught (the gate is
                                                           // checked before the relative-is-inside shortcut).
        assert!(path_outside_workspace("../../etc/cron.d/evil", ws));
    }

    // --- decision matrix (pure, no DB) -------------------------------------

    #[test]
    fn decide_benign_allows_with_empty_grant() {
        let (allowed, _) = decide_unattended(RequestVerdict::Benign, &[]);
        assert!(allowed);
    }

    #[test]
    fn decide_catastrophic_denies_even_when_other_granted() {
        let (allowed, _) =
            decide_unattended(RequestVerdict::Catastrophic, &[SideEffectCategory::Other]);
        assert!(!allowed);
    }

    #[test]
    fn decide_side_effect_allows_only_when_granted() {
        let (allowed, reason) = decide_unattended(
            RequestVerdict::SideEffect(SideEffectCategory::Email),
            &[SideEffectCategory::Email],
        );
        assert!(allowed);
        assert_eq!(reason, UNATTENDED_ALLOW_GRANTED_REASON);

        let (denied, _) = decide_unattended(
            RequestVerdict::SideEffect(SideEffectCategory::Email),
            &[SideEffectCategory::ExternalApi],
        );
        assert!(!denied);
    }

    // --- resolver (DB-backed) ----------------------------------------------

    #[tokio::test]
    async fn resolve_attend_mode_human_device_is_interactive() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        insert_origin_event(
            &pool,
            thread_id,
            "MessageReceived",
            &MessageOrigin::Device {
                device_id: "d1".into(),
                label: "Phone".into(),
            },
        )
        .await;
        let mode = resolve_attend_mode(&pool, &empty_trigger_configs(), thread_id).await;
        assert_eq!(mode, AttendMode::Interactive);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_attend_mode_trigger_root_inherits_grant() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let trigger_id = "trig-1";
        let thread_id = Uuid::new_v4();
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin(trigger_id),
        )
        .await;
        let cfgs = trigger_configs_with(trigger_id, vec![SideEffectCategory::ExternalApi]);
        let mode = resolve_attend_mode(&pool, &cfgs, thread_id).await;
        assert_eq!(
            mode,
            AttendMode::Unattended {
                grant: vec![SideEffectCategory::ExternalApi]
            }
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_attend_mode_walks_agent_subthread_to_trigger_root() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let trigger_id = "trig-2";
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        insert_origin_event(&pool, root, "TriggerStarted", &scheduler_origin(trigger_id)).await;
        insert_child_spawn_event(&pool, child, root).await;
        let cfgs = trigger_configs_with(trigger_id, vec![SideEffectCategory::Email]);
        let mode = resolve_attend_mode(&pool, &cfgs, child).await;
        assert_eq!(
            mode,
            AttendMode::Unattended {
                grant: vec![SideEffectCategory::Email]
            }
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A `relation: "top"` spawn stamps a `ThreadLink` origin naming its
    /// spawning thread (so the route popover can link back) but carries NO
    /// `parent_thread_id`. Attribution must not lend it the spawning thread's trigger
    /// grant: the thread was asked to run independently, so an unresolvable
    /// permission request waits for a human instead of auto-approving a
    /// real-world side effect.
    #[tokio::test]
    async fn resolve_attend_mode_top_spawn_does_not_inherit_trigger_grant() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let trigger_id = "trig-top";
        let root = Uuid::new_v4();
        let spawned = Uuid::new_v4();
        insert_origin_event(&pool, root, "TriggerStarted", &scheduler_origin(trigger_id)).await;
        // Origin without linkage: the top-spawn shape.
        insert_origin_event(&pool, spawned, "MessageReceived", &parent_link(root)).await;
        let cfgs = trigger_configs_with(trigger_id, vec![SideEffectCategory::Email]);
        let mode = resolve_attend_mode(&pool, &cfgs, spawned).await;
        assert_eq!(
            mode,
            AttendMode::Interactive,
            "a top spawn names its spawning thread for display, but is not in its privilege tree"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_attend_mode_human_rooted_subthread_is_interactive() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        insert_origin_event(
            &pool,
            root,
            "MessageReceived",
            &MessageOrigin::Device {
                device_id: "d".into(),
                label: "Mac".into(),
            },
        )
        .await;
        insert_child_spawn_event(&pool, child, root).await;
        let mode = resolve_attend_mode(&pool, &empty_trigger_configs(), child).await;
        assert_eq!(mode, AttendMode::Interactive);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_attend_mode_no_origin_is_interactive() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let mode = resolve_attend_mode(&pool, &empty_trigger_configs(), Uuid::new_v4()).await;
        assert_eq!(mode, AttendMode::Interactive);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // --- unattended prompt path (DB-backed) --------------------------------

    #[tokio::test]
    async fn unattended_trigger_session_auto_allows_benign_without_card() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let trigger_id = "trig-benign";
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "MessageReceived",
            &scheduler_origin(trigger_id),
        )
        .await;
        let cfgs = trigger_configs_with(trigger_id, vec![]);

        // No broadcast is ever fired — if the call WAITED for a card, the timeout
        // would fire and fail the test. That is the no-hang invariant.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &cfgs,
                Path::new("/ws"),
                // Command requests — the worktree fast path never covers them.
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({
                        "command": "/bin/zsh -lc 'lucidos data write artifacts/work-tracker/data.json --from /tmp/x.json'"
                    }),
                },
            ),
        )
        .await
        .expect("unattended resolve must not hang");
        assert!(outcome.allowed, "benign in-workspace write auto-allows");

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(count, 0, "unattended path must not render a card");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn unattended_trigger_denies_ungranted_side_effect() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let trigger_id = "trig-nogrant";
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin(trigger_id),
        )
        .await;
        let cfgs = trigger_configs_with(trigger_id, vec![]); // grants nothing

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &cfgs,
                Path::new("/ws"),
                // Command requests — the worktree fast path never covers them.
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({"command": "/bin/zsh -lc 'curl -X POST https://example.com -d @x'"}),
                },
            ),
        )
        .await
        .expect("must not hang");
        assert!(!outcome.allowed, "ungranted external API auto-denies");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn unattended_trigger_allows_granted_side_effect() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let trigger_id = "trig-grant";
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin(trigger_id),
        )
        .await;
        let cfgs = trigger_configs_with(trigger_id, vec![SideEffectCategory::ExternalApi]);

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &cfgs,
                Path::new("/ws"),
                // Command requests — the worktree fast path never covers them.
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({"command": "/bin/zsh -lc 'curl -X POST https://example.com -d @x'"}),
                },
            ),
        )
        .await
        .expect("must not hang");
        assert!(outcome.allowed, "granted external API auto-allows");
        assert_eq!(
            outcome.reason.as_deref(),
            Some(UNATTENDED_ALLOW_GRANTED_REASON)
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
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
            let trigger_configs = empty_trigger_configs();
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    &trigger_configs,
                    Path::new("/tmp"),
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i1".to_string(),
                        tool_name: "command_execution".to_string(),
                        input: serde_json::json!({"command": "sudo ls"}),
                    },
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
            &empty_trigger_configs(),
            Path::new("/tmp"),
            None,
            CodingAgentPermissionInput {
                thread_id,
                tool_use_id: "i2".to_string(),
                tool_name: "Bash".to_string(),
                input,
            },
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

    /// Count the thread's persisted permission cards.
    async fn card_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("count cards")
    }

    /// A file write inside the session's own worktree resolves with no card —
    /// the `.claude/rules/*.md` edit that used to cost a click on every save.
    #[tokio::test]
    async fn prompt_skips_card_for_in_worktree_write() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let f = worktree_fixture();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &empty_trigger_configs(),
                Path::new("/ws"),
                Some(&f.root),
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i-wt".into(),
                    tool_name: "Edit".into(),
                    input: serde_json::json!({
                        "file_path": under(&f.root, ".claude/rules/frontend.md"),
                        "old_string": "x",
                        "new_string": "y"
                    }),
                },
            ),
        )
        .await
        .expect("in-worktree write must not wait for a card");

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(WORKTREE_WRITE_ALLOW_REASON));
        assert_eq!(
            card_count(&pool, thread_id).await,
            0,
            "the worktree fast path must not render a card"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The negative half: a write OUTSIDE the worktree (the user's global CC
    /// config is the real-world case) still blocks on a card.
    #[tokio::test]
    async fn prompt_renders_card_for_out_of_worktree_write() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let waiter = {
            let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
            let cfgs = empty_trigger_configs();
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    &cfgs,
                    Path::new("/ws"),
                    Some(Path::new("/ws/.lucidos/worktrees/thread-abc")),
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i-out".into(),
                        tool_name: "Edit".into(),
                        input: serde_json::json!({ "file_path": "/home/u/.claude/settings.json" }),
                    },
                )
                .await
            })
        };

        let request_id = await_canonical_request(&pending).await;
        let entry = pending
            .lock()
            .unwrap()
            .take(&request_id)
            .expect("canonical entry present");
        let _ = entry.tx.send(true);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("resolves within 10s")
            .expect("task ok");
        assert!(outcome.allowed);
        assert_eq!(
            outcome.reason, None,
            "an out-of-worktree write must be answered by the user, not a fast path"
        );
        assert_eq!(card_count(&pool, thread_id).await, 1, "one card rendered");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Spin until the canonical entry lands, then hand back its request_id.
    async fn await_canonical_request(pending: &Mutex<PermissionState>) -> String {
        loop {
            let id = {
                let state = pending.lock().unwrap();
                state.by_request_id.keys().next().cloned()
            };
            if let Some(id) = id {
                return id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Persist one request + resolution pair the way the MCP consent endpoint
    /// does, so hydration has something to read.
    async fn seed_resolved_permission(
        bus: &EventBus,
        thread_id: Uuid,
        request_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        allowed: bool,
        persist_scope: Option<crate::engine::claude_code::AllowScope>,
    ) {
        for event in [
            ThreadEvent::CodingAgentPermissionRequest {
                request_id: request_id.to_string(),
                tool_use_id: format!("tu-{request_id}"),
                tool_name: tool_name.to_string(),
                input: input.clone(),
                summary: build_permission_summary(tool_name, &input),
            },
            ThreadEvent::CodingAgentPermissionResolved {
                request_id: request_id.to_string(),
                allowed,
                reason: None,
                persist_scope,
            },
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .expect("seed emit")
            .expect("seed persisted");
        }
    }

    /// The restart case: the grant is durable in the events, so a FRESH
    /// `PermissionState` (what an engine restart leaves behind) must still
    /// suppress the prompt. Before hydration existed, every Apply-with-restart
    /// re-asked for a file the user had already approved on that thread.
    #[tokio::test]
    async fn session_allow_survives_a_fresh_permission_state() {
        use crate::engine::claude_code::AllowScope;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        seed_resolved_permission(
            &bus,
            thread_id,
            "req-granted",
            "Bash",
            serde_json::json!({ "command": "git status" }),
            true,
            Some(AllowScope::Session),
        )
        .await;

        // Fresh state — no in-memory grant survives an engine restart.
        let pending = Arc::new(Mutex::new(PermissionState::default()));

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &empty_trigger_configs(),
                Path::new("/ws"),
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i-after-restart".into(),
                    // Same first token as the grant, different command — the
                    // session pattern is `Bash(git:*)`.
                    tool_name: "Bash".into(),
                    input: serde_json::json!({ "command": "git commit -m wip" }),
                },
            ),
        )
        .await
        .expect("a rehydrated grant must not wait for a card");

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(SESSION_ALLOW_REASON));
        assert_eq!(
            card_count(&pool, thread_id).await,
            1,
            "only the seeded card exists — no new one was rendered"
        );
        assert!(
            pending
                .lock()
                .unwrap()
                .hydrated_threads
                .contains(&thread_id),
            "the thread is marked hydrated so later prompts skip the query"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Hydration must be narrow: an Allow-once (no scope) and a Deny are NOT
    /// standing grants, so a fresh state must still render a card for them.
    #[tokio::test]
    async fn allow_once_and_deny_do_not_hydrate() {
        use crate::engine::claude_code::AllowScope;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        // Allow-once: allowed, but no scope.
        seed_resolved_permission(
            &bus,
            thread_id,
            "req-once",
            "Bash",
            serde_json::json!({ "command": "git status" }),
            true,
            None,
        )
        .await;
        // Denied WITH a session scope — the endpoint never writes this pair,
        // but the filter must reject it on `allowed` regardless.
        seed_resolved_permission(
            &bus,
            thread_id,
            "req-denied",
            "Bash",
            serde_json::json!({ "command": "git push" }),
            false,
            Some(AllowScope::Session),
        )
        .await;

        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let waiter = {
            let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
            let cfgs = empty_trigger_configs();
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    &cfgs,
                    Path::new("/ws"),
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i-again".into(),
                        tool_name: "Bash".into(),
                        input: serde_json::json!({ "command": "git status" }),
                    },
                )
                .await
            })
        };

        let request_id = await_canonical_request(&pending).await;
        let entry = pending
            .lock()
            .unwrap()
            .take(&request_id)
            .expect("a card must be rendered");
        let _ = entry.tx.send(true);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
            .await
            .expect("resolves within 10s")
            .expect("task ok");
        assert_eq!(
            outcome.reason, None,
            "answered by the user, not short-circuited by a bogus hydration"
        );
        assert!(
            !pending
                .lock()
                .unwrap()
                .session_allows
                .contains_key(&thread_id),
            "neither an Allow-once nor a Deny may become a standing grant"
        );

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
        state
            .by_dedup_key
            .insert(dead_key.clone(), entry("req-dead"));
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
