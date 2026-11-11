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
use std::sync::Mutex;
use uuid::Uuid;

use crate::api::SharedEngine;
use crate::engine::claude_code::{GrantOffer, CLAUDE_CODE_COMMAND_TOOL, CODEX_COMMAND_TOOL};
use crate::engine::command_guard::{
    self, unwrap_shell_command, CommandSite, JudgeInput, RiskLane, StaticVerdict, TrustedHeads,
};
use crate::engine::command_judge::JudgeVerdict;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::trusted_repo_scripts::trusted_repo_scripts;
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;

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

/// Reason on an auto-ALLOW of a coding-agent command the command guard allowed
/// (see [`guard_allows_command`]).
pub const COMMAND_GUARD_ALLOW_REASON: &str = "Auto-allowed: the command guard allowed this command";

/// What a pending card's waiters receive when it ends. Shared by all three
/// permission lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAnswer {
    /// A human allowed it.
    Allowed,
    /// A human denied it.
    Denied,
    /// The engine withdrew the card before anyone answered, for the reason
    /// stamped on its resolution. Nobody decided, so no waiter may report it as
    /// a user denial.
    Withdrawn(&'static str),
}

impl PermissionAnswer {
    /// The answer a human's Allow or Deny carries.
    pub fn from_decision(allowed: bool) -> Self {
        if allowed {
            Self::Allowed
        } else {
            Self::Denied
        }
    }
}

/// Grouping key for collapsing identical concurrent permission requests. The
/// canonical input is the serialized `input`, which suffices because the agent
/// re-serializes the same struct each time and produces the same bytes.
pub type DedupKey = (Uuid, String, String);

/// Canonical pending entry, owning the broadcast channel that fans the answer
/// out to every blocked waiter on this `(thread, tool, input)` triple.
/// The entry keeps `tool_name` and `input` beside the `DedupKey`. A lane's
/// consent handler reads them without re-parsing. `grant_offer` is the offer
/// the engine stamped on a coding-agent request, which its consent handler
/// stores. The chat and MCP lanes leave it empty.
pub struct PermissionEntry {
    pub thread_id: Uuid,
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub grant_offer: GrantOffer,
    pub tx: tokio::sync::broadcast::Sender<PermissionAnswer>,
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

    /// The thread a pending card belongs to, without resolving it. The consent
    /// routes weigh the caller's reach over it before they answer.
    pub fn thread_of(&self, request_id: &str) -> Option<Uuid> {
        let key = self.by_request_id.get(request_id)?;
        self.by_dedup_key.get(key).map(|entry| entry.thread_id)
    }

    /// Record a session-allow pattern for a thread. Idempotent: a duplicate
    /// insert is a no-op. The pattern is the one the originating card stamped
    /// for `AllowScope::Session`.
    pub fn allow_session(&mut self, thread_id: Uuid, pattern: String) {
        self.session_allows
            .entry(thread_id)
            .or_default()
            .insert(pattern);
    }

    /// True iff `pattern` is recorded for `thread_id`.
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
        grant_offer: GrantOffer,
    ) -> (
        String,
        tokio::sync::broadcast::Receiver<PermissionAnswer>,
        bool,
    ) {
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
                grant_offer,
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
/// engine-emitted resolution. The pattern is the request's stamped
/// `grant_offer.session`, the one the click stored, so the two cannot drift.
///
/// `r.thread_id = $1` is a **trust boundary, not just a scope filter**. This
/// query turns persisted rows into standing grants, so it must read only rows
/// the engine wrote through `BusEvent::Thread`. A domain event is persisted
/// with a NULL `thread_id`, so a row forged through the emit endpoint can never
/// satisfy it. The boundary holds on BOTH aliases, which is why `q` carries its
/// own `thread_id = $1`: the REQUEST row supplies the pattern.
/// Fails toward asking: a query error leaves the thread unhydrated.
async fn hydrate_session_allows(
    pool: &sqlx::PgPool,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
) {
    {
        let state = pending.lock().unwrap();
        if state.hydrated_threads.contains(&thread_id) {
            return;
        }
    } // Lock released before the await — never hold it across one.

    let rows: Vec<(Option<String>,)> = match sqlx::query_as(
        "SELECT q.payload->'grant_offer'->>'session' \
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

    let patterns: Vec<String> = rows.into_iter().filter_map(|(pattern,)| pattern).collect();

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

/// What a coding-agent *command* request carries. Codex raises these as
/// `command_execution`; Claude Code as `Bash`.
///
/// The three cases are not two. A command whose payload could not be read must
/// never take the fall-through of a request that is no command at all.
enum CommandPayload<'a> {
    /// Not a command request at all.
    NotACommand,
    /// The command text.
    Known(&'a str),
    /// A command request whose `command` field is missing, not a string, or
    /// empty. Nothing can be classified, so it cards.
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

/// True when the command runs in a request `cwd` outside the workspace and
/// destroys something there. Codex states that directory. The command text
/// alone reads a relative path as in-workspace, so neither the static verdict
/// nor the judge can see this.
fn destroys_from_request_cwd(
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
) -> bool {
    let cwd_outside = input
        .get("cwd")
        .and_then(|v| v.as_str())
        .is_some_and(|cwd| path_outside_workspace(cwd, workspace_path));
    cwd_outside
        && matches!(
            coding_agent_command(tool_name, input),
            CommandPayload::Known(cmd)
                if command_guard::destroys_when_started_outside(&unwrap_shell_command(cmd))
        )
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
    // write lands elsewhere. Resolve
    // here too, but only to OVERRIDE a lexical "inside". A dangling link on the
    // chain reads as outside, since the write follows it somewhere unknown.
    // Otherwise, when either side fails to resolve we keep the lexical answer.
    // The ordinary case is a `Write` naming a file that does not exist yet, and
    // calling that outside would card every new file.
    match (
        std::fs::canonicalize(workspace_path),
        canonical_existing_prefix(p),
    ) {
        (_, ExistingPrefix::DanglingLink) => true,
        (Ok(root), ExistingPrefix::Resolved(resolved)) => !resolved.starts_with(&root),
        _ => false,
    }
}

fn has_git_component(p: &Path) -> bool {
    p.components()
        .any(|c| matches!(c, std::path::Component::Normal(name) if name == ".git"))
}

/// A path component that disqualifies a target from the in-worktree fast path:
/// `..`, which cannot be proven contained lexically, or `.git` (see
/// [`path_inside_worktree`]).
fn has_rejected_component(p: &Path) -> bool {
    has_git_component(p)
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// What the longest existing prefix of a path resolves to.
#[derive(Debug, PartialEq, Eq)]
enum ExistingPrefix {
    Resolved(std::path::PathBuf),
    /// An entry on the chain exists but does not resolve: a dangling or
    /// looping symlink. A write follows it, so its destination is unknown.
    DanglingLink,
    /// Nothing along the chain exists.
    Unresolved,
}

/// Canonicalize the longest existing prefix of `p`. A `Write` names a file that
/// does not exist yet, so canonicalizing the target itself fails on exactly the
/// case we most need to classify. Walk up to the nearest ancestor that does
/// resolve. Never walk past an entry that exists but fails to resolve: its
/// parent says nothing about where a write through that link lands.
fn canonical_existing_prefix(p: &Path) -> ExistingPrefix {
    let mut current = Some(p);
    while let Some(candidate) = current {
        if let Ok(real) = std::fs::canonicalize(candidate) {
            return ExistingPrefix::Resolved(real);
        }
        if std::fs::symlink_metadata(candidate).is_ok() {
            return ExistingPrefix::DanglingLink;
        }
        current = candidate.parent();
    }
    ExistingPrefix::Unresolved
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
    let ExistingPrefix::Resolved(resolved) = canonical_existing_prefix(p) else {
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

/// What the command guard's static pass made of one coding-agent request.
///
/// Three cases rather than two, for the same reason [`CommandPayload`] has
/// three. "Not a command" and "a command I could not read" must not collapse,
/// or the second rides the first's fall-through.
enum CommandStatic {
    /// Not a command request. The caller reads its file targets instead.
    NotACommand,
    /// A command request whose text could not be read.
    Unreadable,
    /// What [`command_guard::static_classify`] made of the command.
    Verdict(StaticVerdict),
}

/// Run the command guard's static pass over a coding-agent command request,
/// for the escalation gate ([`guard_allows_command`]).
///
/// Codex wraps every command as `/bin/zsh -lc '<script>'`, so the inner script
/// is what gets classified. Without that a wrapped side-effect hides behind
/// `zsh`.
#[cfg(test)]
fn static_command_verdict(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
) -> CommandStatic {
    static_command_verdict_at(tool_name, input, trusted, None)
}

fn static_command_verdict_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
) -> CommandStatic {
    match coding_agent_command(tool_name, input) {
        CommandPayload::NotACommand => CommandStatic::NotACommand,
        CommandPayload::Unresolved => CommandStatic::Unreadable,
        CommandPayload::Known(cmd) => {
            let synthetic = serde_json::json!({ "command": unwrap_shell_command(cmd) });
            CommandStatic::Verdict(command_guard::static_classify_trusting(
                tn::RUN_BASH,
                &synthetic,
                CommandSite::TextOnly { root },
                trusted,
            ))
        }
    }
}

/// Claude Code's file-reading tools, whose path reaches the prompt tool when
/// it lies outside the worktree.
const FILE_READ_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// Whether a `Read`, `Grep` or `Glob` request reaches a secret path
/// ([`command_guard::mentions_secret_path`]). The path is read alone and
/// joined with its glob, so a search rooted at `~/.lucidos` for
/// `secret-keys/*` counts. A bash command is the static pass's business.
fn file_tool_reads_secret(tool_name: &str, input: &serde_json::Value) -> bool {
    if !FILE_READ_TOOLS.contains(&tool_name) {
        return false;
    }
    let field = |key: &str| input.get(key).and_then(|v| v.as_str());
    let root = field("file_path").or_else(|| field("path")).unwrap_or("");
    let globs = [field("pattern"), field("glob")];
    command_guard::mentions_secret_path(root)
        || globs
            .into_iter()
            .flatten()
            .any(|glob| command_guard::mentions_secret_path(&format!("{root}/{glob}")))
}

/// Whether a classified lane lets a coding-agent command skip its card.
///
/// `None` is every way the classification did not land: an unreadable command,
/// a judge that is off, has no provider, errored or timed out. It cards, which
/// is the behavior that predates this gate.
///
/// `Safe` always allows. `ReversibleDanger` allows only under
/// `coding_agent_auto_answer`, and `reversible_allowed` carries that. The chat
/// lane also runs `ReversibleDanger` unasked, but it snapshots the workspace on
/// a safety ref first. This gate has no such undo: turning auto-answer on
/// accepts that trade (ADR 0005, command guard addenda). `Catastrophic` and
/// `IrreversibleDanger` card whatever the flag says, and nothing here ever
/// auto-denies.
fn lane_allows_escalation(lane: Option<RiskLane>, reversible_allowed: bool) -> bool {
    match lane {
        Some(RiskLane::Safe) => true,
        Some(RiskLane::ReversibleDanger) => reversible_allowed,
        Some(RiskLane::IrreversibleDanger | RiskLane::Catastrophic) | None => false,
    }
}

/// Whether the user turned on coding-agent auto-answer.
///
/// Independent of `command_guard`: turning safety on never means asking less.
/// An unreadable preference resolves OFF, because OFF is the card.
async fn coding_agent_auto_answer(pool: &sqlx::PgPool) -> bool {
    crate::core::prefs::CODING_AGENT_AUTO_ANSWER
        .try_read(pool)
        .await
        .unwrap_or(false)
}

/// Ask the command judge to place one ambiguous command, for the escalation
/// gate.
///
/// `Err` names why no verdict came back, and the caller cards. Deliberately NOT
/// `fallback_classify`, which reads an unrecognised head as benign, where an
/// auto-allow here must rest on a positive verdict.
///
/// Auto-answer on is consent to judge every coding-agent command, and nothing
/// else is: the guard's own switches never let a judge answer a coding agent,
/// so turning safety on never means asking less.
async fn judge_escalation_lane(
    engine: &LucidosEngine,
    ji: &JudgeInput,
    thread_id: Uuid,
    auto_answer: bool,
) -> Result<JudgeVerdict, String> {
    if !auto_answer {
        return Err("auto-answer is off".to_string());
    }
    engine
        .judge_command(ji, thread_id)
        .await
        .map_err(|e| format!("the judge failed ({e})"))
}

/// Whether a coding-agent command resolves without a card.
///
/// Codex raises `command_execution` when its OS sandbox refuses to run the
/// command. On macOS that fires for a whole class of ordinary reads: `/bin/ps`
/// is setuid root and the seatbelt profile will not exec a sugid binary, so
/// every process probe escalated. This places the command through the
/// classifier the Lucidos Agent's chat lane uses (ADR 0002).
///
/// `auto_answer` is `coding_agent_auto_answer`. Off, Codex allows on a static
/// `Safe` only, the judge never runs, and a Claude Code `Bash` request never
/// enters the gate. On, both tools allow on `Safe` or `ReversibleDanger`.
///
/// `engine` is `None` where no judge handle exists. The static half still runs
/// without it.
///
/// Three shapes are refused outright, ahead of any verdict:
///
/// * **A command run as another user**, which the head walk skips past. See
///   [`command_guard::command_escalates_privilege`].
/// * **A shape the Safe fast path REFUSED**, as against one whose head it
///   merely did not recognise. The judge is handed the command text alone, so
///   it cannot see the refusal. `grant_covers_command` holds the same line
///   (ADR 0002). A head in `trusted` is no refusal: it goes to the judge.
/// * **A destruction from a request `cwd` outside the workspace**, which no
///   verdict sees. See [`destroys_from_request_cwd`].
///
/// A secret read settles `IrreversibleDanger` in the static pass, so it cards.
#[allow(clippy::too_many_arguments)]
async fn guard_allows_command(
    engine: Option<&LucidosEngine>,
    tool_name: &str,
    input: &serde_json::Value,
    workspace_path: &Path,
    root: Option<&Path>,
    thread_id: Uuid,
    auto_answer: bool,
    trusted: &TrustedHeads,
) -> bool {
    let in_scope = match tool_name {
        CODEX_COMMAND_TOOL => true,
        CLAUDE_CODE_COMMAND_TOOL => auto_answer,
        _ => false,
    };
    if !in_scope {
        return false;
    }
    let (lane, why) = match static_command_verdict_at(tool_name, input, trusted, root) {
        CommandStatic::Verdict(StaticVerdict::Settled(lane)) => {
            (Some(lane), format!("static verdict {lane:?}"))
        }
        CommandStatic::Verdict(StaticVerdict::SettledIrreversible(category)) => (
            Some(RiskLane::IrreversibleDanger),
            format!("static verdict IrreversibleDanger ({category:?})"),
        ),
        CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) if ji.fast_path_refused => (
            None,
            "a refused shape, so the judge was skipped".to_string(),
        ),
        CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) => match engine {
            Some(engine) => {
                match judge_escalation_lane(engine, &ji, thread_id, auto_answer).await {
                    Ok(v) => (
                        Some(v.lane),
                        format!("judge verdict {:?}: {}", v.lane, v.summary),
                    ),
                    Err(why) => (None, why),
                }
            }
            None => (None, "no judge handle".to_string()),
        },
        CommandStatic::NotACommand | CommandStatic::Unreadable => {
            (None, "an unreadable command".to_string())
        }
    };
    let why = if !lane_allows_escalation(lane, auto_answer) {
        why
    } else if command_privilege_escalation(tool_name, input) {
        "it runs as another user".to_string()
    } else if destroys_from_request_cwd(tool_name, input, workspace_path) {
        "it destroys from a cwd outside the workspace".to_string()
    } else {
        return true;
    };
    crate::log!(
        "[CCPermission] {} cards for thread {}: {}",
        tool_name,
        thread_id,
        why
    );
    false
}

/// Whether this command request reaches for another user's rights.
///
/// Reads the request's own command text rather than taking it from
/// [`static_command_verdict`], which answers about the classification and not
/// about the text. An unreadable command is already a card by then, so `false`
/// here cannot widen anything.
fn command_privilege_escalation(tool_name: &str, input: &serde_json::Value) -> bool {
    match coding_agent_command(tool_name, input) {
        CommandPayload::Known(cmd) => command_guard::command_escalates_privilege(cmd),
        CommandPayload::NotACommand | CommandPayload::Unresolved => false,
    }
}

/// Whether this thread's session-allow set already covers the request.
///
/// A command tool takes the chat lane's rule through the one shared predicate,
/// [`command_guard::grant_covers_command`]: EVERY segment head must be
/// covered, and a code-injecting `VAR=value` preamble is refused outright.
/// Matching a single derived pattern instead let `git status && rm -rf /` ride
/// a grant naming only `git`. Every other tool matches its one derived pattern
/// exactly, as before. No grant covers a secret read, by command or file tool,
/// and no session grant covers a command that may write under `.claude/` or
/// `.git/`.
#[cfg(test)]
fn session_allow_covers(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    session_allow_covers_at(tool_name, input, trusted, None, allowed)
}

fn session_allow_covers_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
    if file_tool_reads_secret(tool_name, input) {
        return false;
    }
    if matches!(tool_name, CLAUDE_CODE_COMMAND_TOOL | CODEX_COMMAND_TOOL) {
        let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
            return false;
        };
        return command_guard::grant_covers_command(
            tool_name,
            command,
            CommandSite::TextOnly { root },
            trusted,
            bare_label_only_for_protected_write(tool_name, input, allowed),
        );
    }
    derive_allow_pattern(tool_name, input, AllowScope::Session, trusted)
        .is_some_and(|p| allowed(&p))
}

/// Whether this workspace's persisted allowlist covers the request. Sibling of
/// [`session_allow_covers`], over `cc-allowed-tools` instead of the per-thread
/// set.
///
/// **The honour rule is [`derive_allow_pattern`] itself**: a stored pattern
/// covers a request only where that function would have PRODUCED it for this
/// same request, at `Broad` or `Narrow`. So the engine is never more permissive
/// than the respawned subprocess will be. `None` at both persisted scopes
/// means no stored line covers the request, and it rules out two cases:
///
///   * a bare `Edit` / `Write` / `NotebookEdit` / `ExitPlanMode` line;
///   * a Codex `file_change`.
///
/// A command takes the same per-segment rule as the session lane, through
/// [`command_guard::grant_covers_command`]. Claude Code and Codex both read the
/// `Bash` and `Bash(<head>:*)` lines, so a line means the same whichever agent
/// asks (ADR 0571). Only the bare `Bash` line covers a command that may write
/// under `.claude/` or `.git/`.
#[cfg(test)]
fn persisted_allow_covers(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    persisted_allow_covers_at(tool_name, input, trusted, None, allowed)
}

fn persisted_allow_covers_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
    if file_tool_reads_secret(tool_name, input) {
        return false;
    }
    if !matches!(tool_name, CLAUDE_CODE_COMMAND_TOOL | CODEX_COMMAND_TOOL) {
        return [AllowScope::Broad, AllowScope::Narrow]
            .into_iter()
            .filter_map(|scope| derive_allow_pattern(tool_name, input, scope, trusted))
            .any(|pattern| allowed(&pattern));
    }
    let Some(command) = input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    command_guard::grant_covers_command(
        CLAUDE_CODE_COMMAND_TOOL,
        command,
        CommandSite::TextOnly { root },
        trusted,
        bare_label_only_for_protected_write(CLAUDE_CODE_COMMAND_TOOL, input, allowed),
    )
}

/// `allowed`, narrowed to the bare `label` line when the command may write
/// under `.claude/` or `.git/`. A head grant never covers such a write.
fn bare_label_only_for_protected_write<'a>(
    label: &'a str,
    input: &serde_json::Value,
    allowed: impl Fn(&str) -> bool + 'a,
) -> impl Fn(&str) -> bool + 'a {
    let protected = crate::engine::claude_code::input_touches_protected_path(label, input);
    move |pattern| (!protected || pattern == label) && allowed(pattern)
}

/// The grants a card offers for this request: each scope's derived pattern,
/// kept only when storing it would cover this same request. So no button
/// stores a line that covers nothing (ADR 0571). Read from the verbatim input,
/// under the trust the request resolved.
#[cfg(test)]
fn grant_offer(tool_name: &str, input: &serde_json::Value, trusted: &TrustedHeads) -> GrantOffer {
    grant_offer_at(tool_name, input, trusted, None)
}

fn grant_offer_at(
    tool_name: &str,
    input: &serde_json::Value,
    trusted: &TrustedHeads,
    root: Option<&Path>,
) -> GrantOffer {
    use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
    let offered = |scope| {
        let pattern = derive_allow_pattern(tool_name, input, scope, trusted)?;
        let only = |p: &str| p == pattern;
        let covers = match scope {
            AllowScope::Session => session_allow_covers_at(tool_name, input, trusted, root, only),
            AllowScope::Broad | AllowScope::Narrow => {
                persisted_allow_covers_at(tool_name, input, trusted, root, only)
            }
        };
        covers.then_some(pattern)
    };
    GrantOffer {
        broad: offered(AllowScope::Broad),
        narrow: offered(AllowScope::Narrow),
        session: offered(AllowScope::Session),
    }
}

/// The copy of a permission request's input that its event stores: masked like
/// a coding agent's tool output, minus the session values only its run knows.
fn permission_event_input(input: &serde_json::Value) -> serde_json::Value {
    let secrets = crate::core::subprocess_secret_values(&[], crate::core::inherited_secret_env());
    let mut masked = input.clone();
    crate::core::redact_subprocess_output_in_json(&mut masked, &secrets);
    masked
}

/// `offer` minus any pattern that carries a secret value. The offer is
/// persisted and fanned out with the event, like its redacted input. Dropping
/// the pattern, rather than masking it, keeps the button and the stored grant
/// identical: that request simply offers no such button.
fn without_secret_patterns(offer: GrantOffer) -> GrantOffer {
    let clean = |pattern: Option<String>| {
        pattern.filter(|p| {
            let value = serde_json::Value::String(p.clone());
            permission_event_input(&value) == value
        })
    };
    GrantOffer {
        broad: clean(offer.broad),
        narrow: clean(offer.narrow),
        session: clean(offer.session),
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

/// The trusted repo scripts among a command request's path heads, resolved
/// from the request's `cwd` against the session's own `worktree_path`.
async fn request_trusted_scripts(
    tool_name: &str,
    input: &serde_json::Value,
    worktree_path: Option<&Path>,
) -> TrustedHeads {
    let CommandPayload::Known(command) = coding_agent_command(tool_name, input) else {
        return TrustedHeads::NONE;
    };
    let cwd = input.get("cwd").and_then(|v| v.as_str()).map(Path::new);
    trusted_repo_scripts(command, cwd, worktree_path).await
}

/// One blocking permission round-trip: the shared core both raise paths drive,
/// CC's MCP HTTP path and the Codex app-server bridge. Every session gets the
/// same gates, whoever started it: the spawn gate already refused a run its
/// cause did not consent to (`agent_session::spawn_gate`). The gates, in order:
///
///   1. **Catastrophic floor.** No grant or preference lifts it.
///   2. **In-worktree write fast path.** A file write inside the session's own
///      worktree needs no card. Checked first because it is pure and DB-free.
///   3. **Session-allow pre-check.** An earlier "Allow for this thread" click
///      whose pattern matches skips the prompt, rehydrated from persisted
///      events so it survives an engine restart.
///   4. **Persisted-allow pre-check.** An earlier "Always allow" click whose
///      pattern is in this workspace's `cc-allowed-tools` skips the prompt.
///   5. **Command gate.** A Codex `command_execution` escape the
///      command guard places as `Safe` skips the card. Under
///      `coding_agent_auto_answer` a `ReversibleDanger` one does too, and a
///      Claude Code `Bash` request enters the gate. See
///      [`guard_allows_command`].
///   6. **Card.** [`park_card`] dedups and emits the card, and the wait on the
///      broadcast is **indefinite**.
///
/// The paired `CodingAgentPermissionResolved` is emitted by the consent
/// endpoint, so it fires once per click rather than once per deduped listener.
///
/// `engine` serves step 5's LLM half. `None` leaves its static half.
pub async fn prompt_coding_agent_permission(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    workspace_path: &Path,
    worktree_path: Option<&Path>,
    engine: Option<&SharedEngine>,
    request: CodingAgentPermissionInput,
) -> PermissionPromptOutcome {
    let CodingAgentPermissionInput {
        thread_id,
        tool_use_id,
        tool_name,
        input,
    } = request;

    // Use the session's trusted worktree for the floor's exemption, while
    // retaining the request's cwd.
    if let CommandPayload::Known(command) = coding_agent_command(&tool_name, &input) {
        let command = if let Some(cwd) = input
            .get("cwd")
            .or_else(|| input.get("workdir"))
            .and_then(|v| v.as_str())
        {
            format!("cd {:?} && {}", cwd, command)
        } else {
            command.to_string()
        };
        if let Some(reason) = command_guard::catastrophic_reason_for_command_at(
            &command,
            worktree_path.or(Some(workspace_path)),
        ) {
            let reason = format!("Refused by the command guard: {reason}");
            record_permission_denial(
                pool,
                event_bus,
                thread_id,
                &tool_use_id,
                &tool_name,
                &input,
                &reason,
            )
            .await;
            return PermissionPromptOutcome {
                allowed: false,
                reason: Some(reason),
            };
        }
    }

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

    // Git and disk reads, only for a path head. Grants read it to cover a
    // trusted repo script by path.
    let trusted = request_trusted_scripts(&tool_name, &input, worktree_path).await;

    let is_session_allowed = {
        let pending = pending.lock().unwrap();
        session_allow_covers_at(
            &tool_name,
            &input,
            &trusted,
            worktree_path.or(Some(workspace_path)),
            |p| pending.matches_session_allow(thread_id, p),
        )
    };
    if is_session_allowed {
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(SESSION_ALLOW_REASON.to_string()),
        };
    }

    // The workspace's own "Always allow" grants, read fresh so a click binds
    // THIS session rather than the next spawn: the file reaches Claude Code as
    // `--allowedTools`, which is frozen for the subprocess's life. Mirrors the
    // chat lane, which reads `agent-allowed-commands` on every prompt.
    let granted = crate::core::grants::patterns(
        &crate::core::grants_dir(workspace_path),
        crate::core::grants::GrantFile::CodingAgentTools,
    );
    if persisted_allow_covers_at(
        &tool_name,
        &input,
        &trusted,
        worktree_path.or(Some(workspace_path)),
        |p| granted.iter().any(|g| g == p),
    ) {
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(PERSISTED_ALLOW_REASON.to_string()),
        };
    }

    // Last gate before the card: classify the coding-agent command. Below every
    // grant on purpose, so a command a grant covers never pays for a judge
    // call.
    let is_command = matches!(
        tool_name.as_str(),
        CODEX_COMMAND_TOOL | CLAUDE_CODE_COMMAND_TOOL
    );
    let auto_answer = is_command && coding_agent_auto_answer(pool).await;
    if guard_allows_command(
        engine.map(|e| &**e),
        &tool_name,
        &input,
        workspace_path,
        worktree_path.or(Some(workspace_path)),
        thread_id,
        auto_answer,
        &trusted,
    )
    .await
    {
        crate::log!(
            "[CCPermission] command auto-allowed for thread {}",
            thread_id
        );
        return PermissionPromptOutcome {
            allowed: true,
            reason: Some(COMMAND_GUARD_ALLOW_REASON.to_string()),
        };
    }

    let offer = without_secret_patterns(grant_offer_at(
        &tool_name,
        &input,
        &trusted,
        worktree_path.or(Some(workspace_path)),
    ));
    let mut answer = park_card(
        pool,
        event_bus,
        pending,
        thread_id,
        tool_use_id,
        tool_name,
        &input,
        offer,
    )
    .await;
    // Wait forever for the user: the user is the rate-limiter.
    outcome_from_permission_recv(answer.recv().await)
}

/// The event meta a card carries: the actor recovered from the thread's last
/// user message. Neither raise path carries a header-borne actor (an MCP
/// subprocess, or a JSON-RPC request from the agent child).
async fn card_event_meta(pool: &sqlx::PgPool, thread_id: Uuid) -> EventMeta {
    match lookup_thread_actor(pool, thread_id).await {
        Some(actor) => EventMeta::with_actor(Some(actor)),
        None => EventMeta::NONE,
    }
}

/// Register a pending card and emit its `CodingAgentPermissionRequest`, then
/// return the receiver its answer arrives on. A duplicate concurrent request
/// attaches to the canonical entry and emits nothing, so one click answers
/// every waiter.
#[allow(clippy::too_many_arguments)]
async fn park_card(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    tool_use_id: String,
    tool_name: String,
    input: &serde_json::Value,
    offer: GrantOffer,
) -> tokio::sync::broadcast::Receiver<PermissionAnswer> {
    let canonical_input = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
    let dedup_key: DedupKey = (thread_id, tool_name.clone(), canonical_input);
    // The dedup key and the pending entry keep the verbatim input, which is what
    // the agent asked to run and what the approval must match. The event is
    // persisted and fanned out over SSE, so it carries a redacted COPY.
    let event_input = permission_event_input(input);
    let summary = build_permission_summary(&tool_name, &event_input);

    let (request_id, rx, is_canonical) = {
        let mut pending = pending.lock().unwrap();
        pending.register_or_attach(
            dedup_key,
            thread_id,
            tool_name.clone(),
            input.clone(),
            offer.clone(),
        )
    };

    if is_canonical {
        let meta = card_event_meta(pool, thread_id).await;
        event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentPermissionRequest {
                        request_id,
                        tool_use_id,
                        tool_name,
                        input: event_input,
                        summary,
                        grant_offer: Some(offer),
                    },
                    meta,
                },
                "[CCPermission] CodingAgentPermissionRequest",
            )
            .await;
    }
    rx
}

/// Record an automatic denial in the thread's timeline, as the ordinary
/// request/resolved pair rather than a new event type.
///
/// An automatic ALLOW emits nothing, deliberately: there is no decision to
/// show. A deny is different. The agent reports a failed step, and without
/// this the user cannot see what the engine refused or why. The two events are
/// emitted back to back, so the card renders already answered and the thread
/// is never left needing attention.
///
/// The command text is redacted for the event exactly as a card redacts it:
/// it is persisted AND fanned out over SSE.
async fn record_permission_denial(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    reason: &str,
) {
    let request_id = Uuid::new_v4().to_string();
    let event_input = permission_event_input(input);
    let summary = build_permission_summary(tool_name, &event_input);
    let meta = card_event_meta(pool, thread_id).await;
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
                    grant_offer: None,
                },
                meta: meta.clone(),
            },
            "[CCPermission] CodingAgentPermissionRequest (auto-deny)",
        )
        .await;
    emit_coding_agent_permission_resolved(
        event_bus,
        thread_id,
        request_id,
        false,
        Some(reason.to_string()),
        None,
        meta,
        "[CCPermission] CodingAgentPermissionResolved (auto-deny)",
    )
    .await;
}

/// Map the broadcast `recv` result for a pending permission into the outcome
/// relayed to the agent. Only a human's Deny reads as `DENIAL_REASON`:
///   * `Withdrawn` names the engine's reason and says nobody decided.
///   * `Err(_)` means the channel CLOSED, which can only be the engine tearing
///     down. Every live resolution path sends before dropping the sender, and
///     `gc_dead_entries` never reaps an entry whose receiver is still awaiting.
///     It carries the neutral `RESTART_INTERRUPT_REASON`.
///
/// Otherwise the agent reads a rejection of an approach nobody rejected.
fn outcome_from_permission_recv(
    recv: Result<PermissionAnswer, tokio::sync::broadcast::error::RecvError>,
) -> PermissionPromptOutcome {
    match recv {
        Ok(PermissionAnswer::Allowed) => PermissionPromptOutcome {
            allowed: true,
            reason: None,
        },
        Ok(PermissionAnswer::Denied) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(DENIAL_REASON.to_string()),
        },
        Ok(PermissionAnswer::Withdrawn(reason)) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(format!(
                "{reason}, not a user decision. Nobody denied this request, so do not read it \
                 as a rejection of your approach."
            )),
        },
        Err(_) => PermissionPromptOutcome {
            allowed: false,
            reason: Some(RESTART_INTERRUPT_REASON.to_string()),
        },
    }
}

/// Record a granted "Always allow" by scope, for the coding-agent lane: the
/// pattern the card stamped for it, never a fresh derivation (ADR 0571).
/// `Session` goes into the in-memory per-thread allow set; `Narrow` / `Broad`
/// into `<workspace>/.lucidos/cc-allowed-tools`. A scope the card did not
/// offer records nothing.
///
/// Both storages bind the CURRENT session. CC reads the file at its next spawn,
/// but the engine's own gate reads it on every prompt. That is what makes an
/// "Always allow" click take effect where it was clicked (ADR 0125).
///
/// Sibling of `command_permission::record_command_allow_grant` and
/// `mcp_permission::record_mcp_allow_grant`, one per lane.
pub fn record_coding_agent_allow_grant(
    engine: &LucidosEngine,
    entry: &PermissionEntry,
    scope: crate::engine::claude_code::AllowScope,
) {
    use crate::engine::claude_code::AllowScope;
    let Some(pattern) = entry.grant_offer.pattern(scope).map(str::to_string) else {
        return;
    };
    match scope {
        AllowScope::Session => {
            let mut pending = engine.pending_cc_permission.lock().unwrap();
            pending.allow_session(entry.thread_id, pattern);
        }
        AllowScope::Narrow | AllowScope::Broad => {
            if let Err(e) = crate::core::grants::append(
                &engine.grants_dir(),
                crate::core::grants::GrantFile::CodingAgentTools,
                &pattern,
            ) {
                crate::log!(
                    "[CCPermission] Failed to persist allow pattern {:?}: {}",
                    pattern,
                    e
                );
            }
        }
    }
}

/// Emit a `CodingAgentPermissionResolved` via the bus. Sibling of
/// `command_permission::emit_command_permission_resolved`, and the one place
/// this lane builds the variant: the answer path, the auto-deny, and every
/// dangling-card sweep all come through here.
#[allow(clippy::too_many_arguments)]
pub async fn emit_coding_agent_permission_resolved(
    event_bus: &EventBus,
    thread_id: Uuid,
    request_id: String,
    allowed: bool,
    reason: Option<String>,
    persist_scope: Option<crate::engine::claude_code::AllowScope>,
    meta: EventMeta,
    log_context: &str,
) {
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentPermissionResolved {
                    request_id,
                    allowed,
                    reason,
                    persist_scope,
                },
                meta,
            },
            log_context,
        )
        .await;
}

/// Answer one coding-agent permission card, whoever answered it.
///
/// This lane's half of the contract `command_permission::resolve_command_permission`
/// documents: the single in-process path, three effects in order, and `false`
/// for a request id nothing is waiting on. The blocked waiter here is the
/// agent's own subprocess handler rather than an in-process loop.
pub async fn resolve_coding_agent_permission(
    engine: &LucidosEngine,
    request_id: String,
    allowed: bool,
    persist_scope: Option<crate::engine::claude_code::AllowScope>,
    actor: Option<MessageOrigin>,
    log_context: &str,
) -> bool {
    let entry = {
        let mut pending = engine.pending_cc_permission.lock().unwrap();
        pending.take(&request_id)
    };
    let Some(entry) = entry else {
        return false;
    };
    // Wake the blocked handler, and every deduped waiter on the same broadcast.
    let _ = entry.tx.send(PermissionAnswer::from_decision(allowed));

    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };
    // A scope on a denial grants nothing: the click said no. Nor does one the
    // card never offered.
    let persist_scope =
        persist_scope.filter(|scope| allowed && entry.grant_offer.pattern(*scope).is_some());
    if let Some(scope) = persist_scope {
        record_coding_agent_allow_grant(engine, &entry, scope);
    }
    emit_coding_agent_permission_resolved(
        &engine.event_bus,
        entry.thread_id,
        request_id,
        allowed,
        reason,
        persist_scope,
        EventMeta::with_actor(actor),
        log_context,
    )
    .await;
    // A human answered the card, so agent messages held behind it go now
    // (ADR 0256).
    if let Some(engine) = engine.try_clone_arc() {
        engine.spawn_held_message_release(entry.thread_id);
    }
    true
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

/// The request ids of every permission card on thread `$1` nobody resolved.
const UNRESOLVED_PERMISSION_REQUESTS_SQL: &str = "SELECT e.payload->>'request_id' \
     FROM events e \
     WHERE e.event_type = 'CodingAgentPermissionRequest' \
       AND e.thread_id = $1 \
       AND NOT EXISTS ( \
         SELECT 1 FROM events r \
         WHERE r.event_type = 'CodingAgentPermissionResolved' \
           AND r.payload->>'request_id' = e.payload->>'request_id' \
       )";

/// Whether `thread_id` has a permission card waiting on a human.
pub(crate) async fn has_pending_permission_card(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS ({UNRESOLVED_PERMISSION_REQUESTS_SQL})"
    ))
    .bind(thread_id)
    .fetch_one(pool)
    .await
}

/// Shared core of the two sweeps that clear every unresolved
/// `CodingAgentPermissionRequest` on this thread as denied: the superseded path
/// and the session-ended path. Mirrors `recover_orphan_cc_permission_requests`
/// but scoped to one thread. Two effects per unresolved request:
///
///   1. Fan a `Withdrawn` out to any still-blocked handler through the
///      in-memory broadcast entry. The subprocess's pending call then returns
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
    reason: &'static str,
    log_label: &str,
) {
    let rows: Vec<(Option<String>,)> = match sqlx::query_as(UNRESOLVED_PERMISSION_REQUESTS_SQL)
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
                let _ = entry.tx.send(PermissionAnswer::Withdrawn(reason));
            }
        }
        emit_coding_agent_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(reason.to_string()),
            None,
            EventMeta::with_actor(actor.clone()),
            log_label,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::EngineReason;
    use std::sync::Arc;

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
        let o = outcome_from_permission_recv(Ok(PermissionAnswer::Allowed));
        assert!(o.allowed);
        assert_eq!(o.reason, None);
    }

    #[test]
    fn outcome_explicit_deny_is_user_denied() {
        let o = outcome_from_permission_recv(Ok(PermissionAnswer::Denied));
        assert!(!o.allowed);
        assert_eq!(o.reason.as_deref(), Some(DENIAL_REASON));
    }

    #[test]
    fn outcome_withdrawn_names_the_reason_and_is_not_user_denied() {
        let o = outcome_from_permission_recv(Ok(PermissionAnswer::Withdrawn(SUPERSEDED_REASON)));
        assert!(!o.allowed);
        let reason = o.reason.unwrap();
        assert!(reason.starts_with(SUPERSEDED_REASON), "{reason}");
        assert!(reason.contains("not a user decision"), "{reason}");
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
    ) -> (
        String,
        tokio::sync::broadcast::Receiver<PermissionAnswer>,
        bool,
    ) {
        let tool_name = key.1.clone();
        state.register_or_attach(
            key,
            Uuid::nil(),
            tool_name,
            serde_json::json!({}),
            GrantOffer::default(),
        )
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
            GrantOffer::default(),
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
        let _ = entry.tx.send(PermissionAnswer::Allowed);

        assert_eq!(rx1.recv().await.unwrap(), PermissionAnswer::Allowed);
        assert_eq!(rx2.recv().await.unwrap(), PermissionAnswer::Allowed);
    }

    fn entry(req_id: &str) -> PermissionEntry {
        let (tx, _rx) = tokio::sync::broadcast::channel(1);
        PermissionEntry {
            grant_offer: Default::default(),
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
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
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

    /// Insert a raw event row carrying an origin. Cheaper than driving a full
    /// `MessageReceived` through the bus and the lifecycle guard.
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

    fn device(device_id: &str) -> MessageOrigin {
        MessageOrigin::Device {
            device_id: device_id.into(),
        }
    }

    fn scheduler_origin(trigger_id: &str) -> MessageOrigin {
        MessageOrigin::engine(EngineReason::Scheduler {
            trigger_id: trigger_id.to_string(),
            trigger_name: None,
            actor_email: None,
        })
    }

    // --- grant offers (pure, no DB) ----------------------------------------

    /// The offer a card stamps, per agent. A card persisted before the field
    /// existed still loads, and writes no key back.
    #[test]
    fn a_card_stamps_its_grant_offer_and_an_old_card_still_loads() {
        let offer =
            |tool: &str, input: serde_json::Value| grant_offer(tool, &input, &TrustedHeads::NONE);
        let codex =
            serde_json::json!({ "command": "/bin/zsh -lc 'cat x; git push'", "cwd": "/ws" });
        assert_eq!(
            offer(CODEX_COMMAND_TOOL, codex),
            GrantOffer {
                broad: Some("Bash".into()),
                narrow: Some("Bash(git:*)".into()),
                session: Some("command_execution(git:*)".into()),
            }
        );
        assert_eq!(
            offer(
                CLAUDE_CODE_COMMAND_TOOL,
                serde_json::json!({ "command": "git push" })
            ),
            GrantOffer {
                broad: Some("Bash".into()),
                narrow: Some("Bash(git:*)".into()),
                session: Some("Bash(git:*)".into()),
            }
        );
        // Two heads: one line cannot cover it, so no narrow or session button.
        let two = serde_json::json!({ "command": "git push && curl -X POST https://example.com" });
        assert_eq!(
            offer(CLAUDE_CODE_COMMAND_TOOL, two),
            GrantOffer {
                broad: Some("Bash".into()),
                ..GrantOffer::default()
            }
        );
        // An untrusted path head: only the bare line covers it.
        let script = serde_json::json!({ "command": "/bin/zsh -lc ./scripts/x.sh", "cwd": "/ws" });
        assert_eq!(
            offer(CODEX_COMMAND_TOOL, script),
            GrantOffer {
                broad: Some("Bash".into()),
                ..GrantOffer::default()
            }
        );
        let edit = serde_json::json!({ "file_path": "/tmp/x.md" });
        assert_eq!(
            offer("Edit", edit),
            GrantOffer {
                session: Some("Edit(/tmp/x.md)".into()),
                ..GrantOffer::default()
            }
        );

        let old = serde_json::json!({
            "type": "CodingAgentPermissionRequest",
            "request_id": "r", "tool_use_id": "t", "tool_name": "Bash",
            "input": { "command": "ls" }, "summary": "s", "grant_head": "ls",
        });
        let event: ThreadEvent = serde_json::from_value(old).unwrap();
        assert!(matches!(
            event,
            ThreadEvent::CodingAgentPermissionRequest {
                grant_offer: None,
                ..
            }
        ));
        assert!(serde_json::to_value(&event)
            .unwrap()
            .get("grant_offer")
            .is_none());
    }

    /// The offer is persisted beside the redacted input, so a pattern that
    /// carries a secret is dropped rather than stamped in the clear.
    #[test]
    fn an_offer_pattern_carrying_a_secret_is_not_stamped() {
        let token = format!("ghp_{}", "a".repeat(36));
        let edit = serde_json::json!({ "file_path": format!("/tmp/{token}/x.md") });
        let offer = without_secret_patterns(grant_offer("Edit", &edit, &TrustedHeads::NONE));
        assert_eq!(offer, GrantOffer::default());
        let plain = serde_json::json!({ "file_path": "/tmp/x.md" });
        let offer = without_secret_patterns(grant_offer("Edit", &plain, &TrustedHeads::NONE));
        assert_eq!(offer.session.as_deref(), Some("Edit(/tmp/x.md)"));
    }

    /// No dead button (ADR 0571): every pattern a card stamps, once its click
    /// stores it, covers that same request when it is raised again.
    #[test]
    fn every_stamped_pattern_covers_its_own_request() {
        use crate::engine::claude_code::AllowScope;
        let trusted =
            TrustedHeads::new([("./scripts/x.sh".to_string(), "scripts/x.sh".to_string())]);
        let codex =
            |c: &str| serde_json::json!({ "command": format!("/bin/zsh -lc '{c}'"), "cwd": "/ws" });
        let claude = |c: &str| serde_json::json!({ "command": c });
        let corpus = [
            (CODEX_COMMAND_TOOL, codex("git status")),
            (
                CODEX_COMMAND_TOOL,
                codex("cat data/pid; ps -p 1; gh run rerun 4"),
            ),
            (CODEX_COMMAND_TOOL, codex("./scripts/x.sh --check")),
            (CODEX_COMMAND_TOOL, codex("./scripts/harden-gates.sh")),
            (CODEX_COMMAND_TOOL, codex("git status && ./scripts/x.sh")),
            (CODEX_COMMAND_TOOL, codex("echo x > .git/hooks/pre-commit")),
            (
                CODEX_COMMAND_TOOL,
                codex("timeout 5 curl https://example.com"),
            ),
            (CODEX_COMMAND_TOOL, codex("/usr/bin/git push")),
            (CLAUDE_CODE_COMMAND_TOOL, claude("git push origin main")),
            (
                CLAUDE_CODE_COMMAND_TOOL,
                claude("cat .claude/settings.json"),
            ),
            (CLAUDE_CODE_COMMAND_TOOL, claude("./scripts/x.sh")),
            (CLAUDE_CODE_COMMAND_TOOL, claude("cd sub && ls")),
            (
                "Skill",
                serde_json::json!({ "skill": "code-review:code-review" }),
            ),
            ("Edit", serde_json::json!({ "file_path": "/tmp/x.md" })),
            ("Read", serde_json::json!({ "file_path": "/tmp/x.md" })),
            (
                "file_change",
                serde_json::json!({ "changes": [{ "path": "/tmp/x" }] }),
            ),
        ];
        let mut stamped = 0;
        for (tool, input) in &corpus {
            let offer = grant_offer(tool, input, &trusted);
            for scope in [AllowScope::Broad, AllowScope::Narrow, AllowScope::Session] {
                let Some(pattern) = offer.pattern(scope) else {
                    continue;
                };
                stamped += 1;
                let stored = |p: &str| p == pattern;
                let covered = match scope {
                    AllowScope::Session => session_allow_covers(tool, input, &trusted, stored),
                    _ => persisted_allow_covers(tool, input, &trusted, stored),
                };
                assert!(covered, "{tool} {input} {scope:?} stamped dead {pattern}");
            }
        }
        assert!(stamped > corpus.len(), "the corpus stamps buttons at all");
        // The bug that started ADR 0571: a trusted script gets its path button.
        let offer = grant_offer(CODEX_COMMAND_TOOL, &corpus[2].1, &trusted);
        assert_eq!(offer.narrow.as_deref(), Some("Bash(./scripts/x.sh:*)"));
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

    // --- command gate --------------------------------------------------------

    /// Run the gate with no judge handle, which is the static half alone. Every
    /// case below is one the static pass settles, so the missing handle is the
    /// point rather than a shortcut. `coding_agent_auto_answer` is off.
    async fn escalation(tool_name: &str, input: serde_json::Value) -> bool {
        command_gate(tool_name, input, false).await
    }

    async fn command_gate(tool_name: &str, input: serde_json::Value, auto_answer: bool) -> bool {
        guard_allows_command(
            None,
            tool_name,
            &input,
            Path::new("/ws"),
            None,
            Uuid::new_v4(),
            auto_answer,
            &TrustedHeads::NONE,
        )
        .await
    }

    /// Codex escalates when its sandbox refuses a write outside the worktree.
    /// The fast path settles a relative overwrite Safe without the `cwd`, so
    /// the gate reads the `cwd` itself before it skips the card.
    #[tokio::test]
    async fn an_overwrite_from_a_cwd_outside_the_worktree_still_cards() {
        for cmd in ["echo x > .zshrc", "rm -rf old"] {
            let outside = serde_json::json!({ "command": cmd, "cwd": "/Users/me" });
            assert!(!escalation("command_execution", outside).await, "{cmd}");
        }
        let read = serde_json::json!({ "command": "cat .zshrc", "cwd": "/Users/me" });
        assert!(escalation("command_execution", read).await);
    }

    fn zsh(script: &str) -> serde_json::Value {
        serde_json::json!({ "command": format!("/bin/zsh -lc '{script}'") })
    }

    #[test]
    fn only_the_safe_lane_skips_the_card_with_the_flag_off() {
        assert!(lane_allows_escalation(Some(RiskLane::Safe), false));
        assert!(
            !lane_allows_escalation(Some(RiskLane::ReversibleDanger), false),
            "this gate has no safety ref, so in-workspace destruction still asks"
        );
        assert!(!lane_allows_escalation(
            Some(RiskLane::IrreversibleDanger),
            false
        ));
        assert!(!lane_allows_escalation(Some(RiskLane::Catastrophic), false));
        assert!(
            !lane_allows_escalation(None, false),
            "nothing classified it, so the user does"
        );
    }

    #[test]
    fn the_flag_adds_the_reversible_lane_and_nothing_else() {
        assert!(lane_allows_escalation(Some(RiskLane::Safe), true));
        assert!(lane_allows_escalation(
            Some(RiskLane::ReversibleDanger),
            true
        ));
        assert!(!lane_allows_escalation(
            Some(RiskLane::IrreversibleDanger),
            true
        ));
        assert!(!lane_allows_escalation(Some(RiskLane::Catastrophic), true));
        assert!(!lane_allows_escalation(None, true));
    }

    /// The shape that made Codex unusable: `/bin/ps` is setuid root, so codex's
    /// seatbelt refuses to exec it and every process probe escalated.
    #[tokio::test]
    async fn a_process_probe_needs_no_card() {
        for script in [
            "ps aux | grep python | grep -v grep",
            "cat /tmp/run.log",
            "sleep 20; wc -c /tmp/run.log",
        ] {
            assert!(
                escalation("command_execution", zsh(script)).await,
                "read-only heads are on the guard's static allowlist: {script}"
            );
        }
    }

    #[tokio::test]
    async fn a_catastrophic_command_still_cards() {
        assert!(!escalation("command_execution", zsh("rm -rf /")).await);
    }

    #[tokio::test]
    async fn an_unreadable_command_still_cards() {
        for input in [
            serde_json::json!({}),
            serde_json::json!({ "command": "" }),
            serde_json::json!({ "command": 7 }),
        ] {
            assert!(
                !escalation("command_execution", input).await,
                "nothing can be classified, so nothing is auto-allowed"
            );
        }
    }

    /// A shape the Safe fast path REFUSED never reaches the judge, so it cards
    /// whether or not a judge handle exists. The judge sees only the command
    /// text. It cannot tell a refusal from an ordinary read, and would answer
    /// `safe` on the words alone.
    ///
    /// `escalation` passes no handle, which is why each case below asserts the
    /// refusal through `static_command_verdict` as well: without that, a
    /// regression putting these back on the judge's path would still pass.
    #[tokio::test]
    async fn a_refused_shape_never_reaches_the_judge() {
        for script in [
            "PATH=bin ls",
            "./scripts/deploy.sh",
            "echo hi >> /etc/hosts",
            "cat $(whoami)",
        ] {
            let input = zsh(script);
            let refused = matches!(
                static_command_verdict("command_execution", &input, &TrustedHeads::NONE),
                CommandStatic::Verdict(StaticVerdict::NeedsJudge(ref ji)) if ji.fast_path_refused
            );
            assert!(refused, "fixture must be a fast-path refusal: {script}");
            assert!(
                !escalation("command_execution", input).await,
                "a refusal is never waved through: {script}"
            );
        }
    }

    /// What Codex sent for a handoff: a literal heredoc written to the temp
    /// dir, with code spans in the body. Neither part is a refused shape, so
    /// the attended gate asks the judge rather than carding.
    #[test]
    fn a_scratch_handoff_through_a_literal_heredoc_reaches_the_judge() {
        let command = "/bin/zsh -lc \"cat > /private/tmp/handoff.md <<'HANDOFF'\n\
            # Outcomes\n\n\
            Phase 3 is committed as \"'`feat: add x`. Run `make lint` > log next.'\"\n\
            HANDOFF\n\
            lucidos successor start --narrative /private/tmp/handoff.md --at phase-commit --progress 4/7\"";
        let input = serde_json::json!({ "command": command, "cwd": "/ws" });
        assert!(matches!(
            static_command_verdict(CODEX_COMMAND_TOOL, &input, &TrustedHeads::NONE),
            CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) if !ji.fast_path_refused
        ));
    }

    /// `sudo` is a benign PREFIX to the head walk, so `sudo cat` settles `Safe`
    /// on the strength of `cat`. Running it outside the sandbox as root is not
    /// the act the verdict described.
    #[tokio::test]
    async fn a_privileged_command_still_cards() {
        for script in [
            "sudo cat /etc/shadow",
            "doas ls /root",
            "ls && sudo cat /etc/shadow",
            "/usr/bin/sudo cat /etc/shadow",
        ] {
            assert!(
                !escalation("command_execution", zsh(script)).await,
                "reaching for another user's rights asks: {script}"
            );
        }
    }

    /// The other side of the rule above: a privilege word as an ARGUMENT is an
    /// ordinary read and keeps its auto-allow.
    #[tokio::test]
    async fn a_privilege_word_in_an_argument_is_not_an_escalation() {
        assert!(escalation("command_execution", zsh("grep sudo /etc/hosts")).await);
    }

    /// An unrecognised head is the ambiguous middle. This gate does not read it
    /// as benign, because an auto-allow rests on a verdict rather than on
    /// silence.
    #[tokio::test]
    async fn an_unrecognised_head_cards_without_a_judge() {
        assert!(!escalation("command_execution", zsh("cargo build")).await);
    }

    #[tokio::test]
    async fn the_gate_covers_only_command_tools() {
        for tool in ["file_change", "Edit", "Skill"] {
            for flag in [false, true] {
                assert!(
                    !command_gate(tool, zsh("ps aux"), flag).await,
                    "{tool} keeps the behavior it had before the gate (flag {flag})"
                );
            }
        }
    }

    /// With the flag off a Claude Code `Bash` card never enters the gate, so it
    /// cards exactly as it did before this gate covered it.
    #[tokio::test]
    async fn a_bash_card_asks_with_the_flag_off() {
        assert!(!command_gate("Bash", zsh("ps aux"), false).await);
        assert!(command_gate("command_execution", zsh("ps aux"), false).await);
    }

    #[tokio::test]
    async fn a_bash_card_is_classified_with_the_flag_on() {
        assert!(command_gate("Bash", zsh("ps aux"), true).await);
        assert!(command_gate("Bash", serde_json::json!({ "command": "ls -la" }), true).await);
    }

    /// The refusals and the always-card lanes hold for both tool names with the
    /// flag on.
    #[tokio::test]
    async fn the_refusals_hold_with_the_flag_on() {
        let cases = [
            ("rm -rf /", "catastrophic"),
            ("sudo cat /etc/shadow", "privilege escalation"),
            ("ls && sudo cat /etc/shadow", "privilege escalation"),
            ("./scripts/deploy.sh", "fast-path refusal"),
            ("cat ~/.lucidos/secret-keys/db", "secret read"),
            ("curl -X POST https://example.com -d @x", "unjudged middle"),
        ];
        for tool in ["command_execution", "Bash"] {
            for (script, why) in cases {
                assert!(
                    !command_gate(tool, zsh(script), true).await,
                    "{tool} still asks on {why}: {script}"
                );
            }
            let outside = serde_json::json!({ "command": "rm -rf old", "cwd": "/Users/me" });
            assert!(
                !command_gate(tool, outside, true).await,
                "{tool}: a destruction from a cwd outside the workspace still asks"
            );
        }
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
    }

    #[test]
    fn a_change_set_of_an_unknown_shape_never_reaches_the_worktree_fast_path() {
        // `changes` present but not a list is a shape we did not understand.
        // The fast path must not fall through to `grant_root` and treat that
        // directory as the request's one known target.
        let f = worktree_fixture();
        let inside =
            serde_json::json!({ "changes": "surprise", "grant_root": under(&f.root, "sub") });
        assert!(!worktree_write_auto_allowed(
            "file_change",
            &inside,
            Some(&f.root)
        ));
        let input = serde_json::json!({ "changes": "surprise", "grant_root": "/ws/sub" });
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
    /// check, so the write lands outside.
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

    /// A link to a missing target fails to canonicalize. The walk used to step
    /// past it to the worktree root and call it contained, while a write
    /// through it creates the target outside.
    #[test]
    #[cfg(unix)]
    fn a_dangling_symlink_is_never_read_as_contained() {
        let f = worktree_fixture();
        std::os::unix::fs::symlink(f.outside.join("not-yet.plist"), f.root.join("dangling"))
            .expect("symlink");
        let link = under(&f.root, "dangling");
        assert!(!path_inside_worktree(&link, &f.root));
        assert!(path_outside_workspace(&link, &f.root));
        assert_eq!(
            canonical_existing_prefix(Path::new(&link)),
            ExistingPrefix::DanglingLink
        );
    }

    // --- grants and secrets (pure, no DB) ----------------------------------

    /// Secret reads by every coding-agent tool that can make one.
    fn secret_read_requests() -> Vec<(&'static str, serde_json::Value)> {
        use serde_json::json;
        vec![
            (
                "Bash",
                json!({ "command": "cat ~/.lucidos/secret-keys/db" }),
            ),
            (
                "command_execution",
                json!({ "command": "/bin/zsh -lc 'cat $HOME/.lucidos/pg-shared-password'" }),
            ),
            (
                "Read",
                json!({ "file_path": "/Users/me/.lucidos/secret-keys/db" }),
            ),
            (
                "Grep",
                json!({ "pattern": "x", "path": "/Users/me/.lucidos/secret-keys" }),
            ),
            (
                "Glob",
                json!({ "pattern": "secret-keys/*", "path": "/Users/me/.lucidos" }),
            ),
            (
                "Glob",
                json!({ "pattern": "/home/u/.lucidos/pg-shared-password" }),
            ),
        ]
    }

    #[test]
    fn a_permission_event_stores_its_input_masked() {
        let input = serde_json::json!({
            "command": "curl -H 'Authorization: Bearer ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d' \
                        && psql postgres://lucidos:hunter2@localhost/db",
        });
        assert_eq!(
            permission_event_input(&input),
            serde_json::json!({
                "command": "curl -H 'Authorization: Bearer ghp_[REDACTED]' \
                            && psql postgres://lucidos:***@localhost/db",
            })
        );
    }

    #[test]
    fn no_stored_grant_covers_a_secret_read_in_the_interactive_lane() {
        let all = |_: &str| true;
        for (tool, input) in secret_read_requests() {
            assert!(
                !session_allow_covers(tool, &input, &TrustedHeads::NONE, all),
                "{tool}: {input}"
            );
            assert!(
                !persisted_allow_covers(tool, &input, &TrustedHeads::NONE, all),
                "{tool}: {input}"
            );
        }
    }

    /// The three commands a bare `Bash` grant still carded, though each only
    /// reads under `.claude/`: a sed that prints, a loop, a substitution.
    #[test]
    fn a_bare_bash_grant_covers_a_read_through_sed_a_loop_or_a_substitution() {
        let bare_bash = |p: &str| p == CLAUDE_CODE_COMMAND_TOOL;
        for command in [
            "rg -n x .claude/rules | sed \"s|a||\"",
            "R=/repo; rg -n x .claude/rules | sed \"s|$R/||\"",
            "for f in a .claude/rules/x.md:84; do ln=${f##*:}; line=$(sed -n \"${ln}p\" \"$f\"); done",
        ] {
            let input = serde_json::json!({ "command": command });
            assert!(
                persisted_allow_covers(
                    CLAUDE_CODE_COMMAND_TOOL,
                    &input,
                    &TrustedHeads::NONE,
                    bare_bash
                ),
                "{command}"
            );
        }
    }

    #[tokio::test]
    async fn a_secret_read_escalation_still_cards() {
        let input =
            serde_json::json!({ "command": "/bin/zsh -lc 'cat ~/.lucidos/secret-keys/db'" });
        for flag in [false, true] {
            assert!(
                !guard_allows_command(
                    None,
                    CODEX_COMMAND_TOOL,
                    &input,
                    Path::new("/ws"),
                    None,
                    Uuid::new_v4(),
                    flag,
                    &TrustedHeads::NONE
                )
                .await
            );
        }
    }

    #[test]
    fn a_neighbouring_file_read_is_not_a_secret_read() {
        use serde_json::json;
        for (tool, input) in [
            ("Read", json!({ "file_path": "/Users/me/.lucidos/ports" })),
            (
                "Grep",
                json!({ "pattern": "secret-keys", "path": "/ws/docs" }),
            ),
            (
                "Glob",
                json!({ "pattern": "**/*.rs", "path": "/Users/me/.lucidos" }),
            ),
        ] {
            assert!(!file_tool_reads_secret(tool, &input), "{tool}: {input}");
        }
    }

    /// The command gate hands a trusted repo script to the judge. A grant
    /// covers it by path, and only when nothing runs before it: an earlier
    /// segment could rewrite the script (ADR 0571).
    #[test]
    fn a_trusted_repo_script_is_judged_and_granted_only_first() {
        let trusted =
            TrustedHeads::new([("./scripts/x.sh".to_string(), "scripts/x.sh".to_string())]);
        let codex = |script: &str| serde_json::json!({ "command": format!("/bin/zsh -lc '{script}'"), "cwd": "/ws" });
        let late = codex("git status && ./scripts/x.sh");
        assert!(matches!(
            static_command_verdict(CODEX_COMMAND_TOOL, &late, &trusted),
            CommandStatic::Verdict(StaticVerdict::NeedsJudge(ji)) if !ji.fast_path_refused
        ));
        let all = |p: &str| p.starts_with("command_execution(") || p.starts_with("Bash(");
        assert!(!session_allow_covers(
            CODEX_COMMAND_TOOL,
            &late,
            &trusted,
            all
        ));
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &late,
            &trusted,
            all
        ));

        let first = codex("./scripts/x.sh --check");
        let path_line = |p: &str| p == "Bash(./scripts/x.sh:*)";
        assert!(persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &trusted,
            path_line
        ));
        assert!(session_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &trusted,
            |p| { p == "command_execution(./scripts/x.sh:*)" }
        ));
        // Untrusted, the same line covers nothing.
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &TrustedHeads::NONE,
            path_line
        ));
        // A basename line never covers a path head.
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &first,
            &trusted,
            |p| { p == "Bash(x.sh:*)" }
        ));
    }

    /// A bare `Bash` grant covers a command that only names or reads
    /// `.claude/` or `.git/`. Both shapes carded on every call once the engine
    /// matched Bash grants itself (ADR 0634).
    #[test]
    fn a_bash_grant_covers_a_protected_path_mention() {
        let bash = |p: &str| p == "Bash";
        for command in [
            "N=\"${TMPDIR:-/tmp}/handoff.md\"; cat > \"$N\" <<'EOF'\nSee `.claude/rules/dev-runtime.md`.\nEOF",
            "find . -name '*.rs' -not -path '*/.git/*' | head",
        ] {
            let input = serde_json::json!({ "command": command });
            assert!(
                persisted_allow_covers("Bash", &input, &TrustedHeads::NONE, bash),
                "{command}"
            );
            assert!(
                persisted_allow_covers(CODEX_COMMAND_TOOL, &input, &TrustedHeads::NONE, bash),
                "{command}"
            );
            assert_eq!(
                grant_offer("Bash", &input, &TrustedHeads::NONE).broad.as_deref(),
                Some("Bash"),
                "{command}"
            );
        }
    }

    /// A write under `.claude/` or `.git/` is covered by the bare `Bash` line
    /// only. A head grant, persisted or session, never covers it: `cat` with a
    /// redirect could plant a git hook. Its card offers "Always allow" alone.
    #[test]
    fn only_a_bare_bash_grant_covers_a_protected_path_write() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let cmd = |c: &str| serde_json::json!({ "command": c });
        for c in [
            "for f in .claude/x; do awk '{print}' \"$f\"; done",
            "echo x > .claude/y",
            "cat x > .git/hooks/pre-commit",
        ] {
            for tool in [CLAUDE_CODE_COMMAND_TOOL, CODEX_COMMAND_TOOL] {
                assert!(
                    persisted_allow_covers(tool, &cmd(c), &TrustedHeads::NONE, granted(&["Bash"])),
                    "bare Bash must cover {c} for {tool}"
                );
            }
            let offer = grant_offer("Bash", &cmd(c), &TrustedHeads::NONE);
            assert_eq!(offer.broad.as_deref(), Some("Bash"), "{c}");
            assert_eq!(offer.narrow, None, "{c}");
            assert_eq!(offer.session, None, "{c}");
        }
        let hook = cmd("cat x > .git/hooks/pre-commit");
        assert!(!persisted_allow_covers(
            "Bash",
            &hook,
            &TrustedHeads::NONE,
            granted(&["Bash(cat:*)"])
        ));
        assert!(!persisted_allow_covers(
            CODEX_COMMAND_TOOL,
            &hook,
            &TrustedHeads::NONE,
            granted(&["Bash(cat:*)"])
        ));
        assert!(!session_allow_covers(
            "Bash",
            &hook,
            &TrustedHeads::NONE,
            granted(&["Bash(cat:*)"])
        ));
        assert!(!session_allow_covers(
            CODEX_COMMAND_TOOL,
            &hook,
            &TrustedHeads::NONE,
            granted(&["command_execution(cat:*)"])
        ));
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
            &TrustedHeads::NONE,
            granted(&["command_execution(git:*)"])
        ));
        // The click that granted `git` must not carry a later `rm`.
        assert!(!session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'rm -rf /'"),
            &TrustedHeads::NONE,
            granted(&["command_execution(git:*)"])
        ));
        // Nor a compound whose trailing segment it never named.
        assert!(!session_allow_covers(
            "command_execution",
            &req("/bin/zsh -lc 'git status && rm -rf /'"),
            &TrustedHeads::NONE,
            granted(&["command_execution(git:*)"])
        ));
        // A code-injecting preamble is refused on the grant lane too.
        assert!(!session_allow_covers(
            "Bash",
            &req("LD_PRELOAD=/tmp/evil.so ls"),
            &TrustedHeads::NONE,
            granted(&["Bash(ls:*)"])
        ));
        // Non-command tools keep their exact single-pattern match.
        assert!(session_allow_covers(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md" }),
            &TrustedHeads::NONE,
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
            &TrustedHeads::NONE,
            granted(&["Bash"])
        ));
        // Narrow is head-scoped, and the head is all it carries.
        assert!(persisted_allow_covers(
            "Bash",
            &cmd("git status"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("rm -rf /"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        // Broad on a tool with no sub-scope is the bare name.
        assert!(persisted_allow_covers(
            "Read",
            &serde_json::json!({ "file_path": "/tmp/foo.md" }),
            &TrustedHeads::NONE,
            granted(&["Read"])
        ));
        // A Skill click can store either shape, and both must match.
        let skill = serde_json::json!({ "skill": "code-review:code-review" });
        assert!(persisted_allow_covers(
            "Skill",
            &skill,
            &TrustedHeads::NONE,
            granted(&["Skill"])
        ));
        assert!(persisted_allow_covers(
            "Skill",
            &skill,
            &TrustedHeads::NONE,
            granted(&["Skill(code-review:*)"])
        ));
        // An empty allowlist covers nothing, which is the first-run state.
        assert!(!persisted_allow_covers(
            "Read",
            &skill,
            &TrustedHeads::NONE,
            granted(&[])
        ));
    }

    /// The shapes a stored pattern must never reach.
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
                    &TrustedHeads::NONE,
                    granted(&["Edit", "Write", "NotebookEdit", "ExitPlanMode"])
                ),
                "a bare {tool} line must not cover its own request"
            );
        }
        // A command writing under a protected path, under its head's grant.
        for c in ["rm -rf .git/hooks", "echo '{}' > .claude/settings.json"] {
            assert!(
                !persisted_allow_covers(
                    "Bash",
                    &cmd(c),
                    &TrustedHeads::NONE,
                    granted(&["Bash(rm:*)", "Bash(echo:*)"])
                ),
                "a protected-path command must still ask: {c}"
            );
        }
        // A Codex card writes only `Bash` lines, so no other line answers for
        // a Codex request.
        assert!(!persisted_allow_covers(
            "command_execution",
            &cmd("/bin/zsh -lc 'git status'"),
            &TrustedHeads::NONE,
            granted(&["command_execution", "command_execution(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            "file_change",
            &serde_json::json!({ "changes": [{ "path": "/tmp/x" }] }),
            &TrustedHeads::NONE,
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
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        // A code-injecting preamble is a refusal, not a `ls` the grant covers.
        assert!(!persisted_allow_covers(
            "Bash",
            &cmd("LD_PRELOAD=/tmp/evil.so ls"),
            &TrustedHeads::NONE,
            granted(&["Bash(ls:*)"])
        ));
        // A command with no readable text has no head to cover.
        assert!(!persisted_allow_covers(
            "Bash",
            &serde_json::json!({}),
            &TrustedHeads::NONE,
            granted(&["Bash(ls:*)"])
        ));
    }

    /// A Codex command reads the `Bash(<head>:*)` line its own card writes, the
    /// same line a Claude Code card writes, under the same per-segment rule.
    #[test]
    fn a_persisted_bash_grant_covers_a_codex_command_segment_by_segment() {
        let granted = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        let codex = |c: &str| serde_json::json!({ "command": c, "cwd": "/ws" });
        let tool = CODEX_COMMAND_TOOL;

        assert!(persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'git status'"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'git status && rm -rf x'"),
            &TrustedHeads::NONE,
            granted(&["Bash(git:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'rm -rf /'"),
            &TrustedHeads::NONE,
            granted(&["Bash(rm:*)"])
        ));
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'echo x > .git/hooks/pre-commit'"),
            &TrustedHeads::NONE,
            granted(&["Bash(echo:*)"])
        ));
        // A path head is a refusal no grant covers, trusted or not.
        assert!(!persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc './x.sh'"),
            &TrustedHeads::NONE,
            granted(&["Bash(x.sh:*)", "Bash(./x.sh:*)"])
        ));
        // A bare `Bash` line covers any command, Codex's too (ADR 0571).
        assert!(persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'cargo test && git push'"),
            &TrustedHeads::NONE,
            granted(&["Bash"])
        ));
        // A protected-path write included.
        assert!(persisted_allow_covers(
            tool,
            &codex("/bin/zsh -lc 'echo x > .git/hooks/pre-commit'"),
            &TrustedHeads::NONE,
            granted(&["Bash"])
        ));
        // Except the two exceptions: a secret read and a catastrophic command.
        for c in ["cat ~/.lucidos/secret-keys/db", "rm -rf /"] {
            assert!(
                !persisted_allow_covers(
                    tool,
                    &codex(&format!("/bin/zsh -lc '{c}'")),
                    &TrustedHeads::NONE,
                    granted(&["Bash"])
                ),
                "bare Bash must not cover {c}"
            );
        }
    }

    /// One line, one meaning (ADR 0571): whether a stored line covers a
    /// command is the same for Claude Code `Bash` and Codex
    /// `command_execution`. Codex wraps its commands, Claude Code does not.
    #[test]
    fn a_persisted_line_means_the_same_for_both_agents() {
        let lines: &[&[&str]] = &[
            &["Bash"],
            &["Bash(git:*)"],
            &["Bash(rm:*)"],
            &["Bash(echo:*)"],
            &["Bash(git:*)", "Bash(curl:*)"],
            &["command_execution"],
            &[],
        ];
        let commands = [
            "git status",
            "cargo test",
            "git push && curl -X POST https://example.com",
            "cat x; git push",
            "rm -rf /",
            "cat ~/.lucidos/secret-keys/db",
            "echo x > .git/hooks/pre-commit",
            "cat .claude/settings.json",
            "LD_PRELOAD=/tmp/evil.so ls",
        ];
        for set in lines {
            let allowed = |p: &str| set.contains(&p);
            for command in commands {
                let claude = persisted_allow_covers(
                    CLAUDE_CODE_COMMAND_TOOL,
                    &serde_json::json!({ "command": command }),
                    &TrustedHeads::NONE,
                    allowed,
                );
                let codex = persisted_allow_covers(
                    CODEX_COMMAND_TOOL,
                    &serde_json::json!({
                        "command": format!("/bin/zsh -lc '{command}'"),
                        "cwd": "/ws",
                    }),
                    &TrustedHeads::NONE,
                    allowed,
                );
                assert_eq!(claude, codex, "{set:?} on {command}");
            }
        }
    }

    #[tokio::test]
    async fn cleanup_floor_precedes_broad_session_grants_on_coding_agent_gate() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("protected");
        std::fs::create_dir_all(root.join(".lucidos")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        let pending = Mutex::new(PermissionState::default());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        pending
            .lock()
            .unwrap()
            .allow_session(thread_id, "Bash".to_string());
        for tool in ["Bash", "command_execution"] {
            let outcome = prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                temp.path(),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: tool.to_string(),
                    tool_name: tool.to_string(),
                    input: serde_json::json!({"command": "rm -rf protected", "cwd": temp.path()}),
                },
            )
            .await;
            assert!(!outcome.allowed, "{tool}");
        }
        let worktree = temp.path().join("own-worktree");
        let gitdir = temp.path().join("git-admin");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "..").unwrap();
        std::fs::create_dir_all(worktree.join(".lucidos")).unwrap();
        std::fs::create_dir_all(worktree.join("data")).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();
        let outcome = prompt_coding_agent_permission(&pool, &bus, &pending,
            temp.path(), Some(&worktree), None, CodingAgentPermissionInput { thread_id,
                tool_use_id: "own-worktree".into(), tool_name: "Bash".into(),
                input: serde_json::json!({"command": format!("rm -rf {}", worktree.display()), "cwd": worktree})
            }).await;
        assert!(outcome.allowed, "the session's own worktree is exempt");
        assert!(root.exists());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // --- prompt path (DB-backed) -------------------------------------------

    /// The whole point of the gate, end to end: codex asks to escape its
    /// sandbox for a process probe, and no card is raised.
    #[tokio::test]
    async fn a_classified_escalation_needs_no_card() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: zsh("ps aux | grep bench | grep -v grep"),
                },
            ),
        )
        .await
        .expect("a classified escalation must not wait for a card");

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(COMMAND_GUARD_ALLOW_REASON));
        assert_eq!(
            count_permission_events(&pool, thread_id).await,
            (0, 0),
            "an auto-allow renders no card, like every other fast path here"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Auto-answer defaults off and never reads the master guard: turning
    /// safety on never answers a card the user did not hand over.
    #[tokio::test]
    async fn auto_answer_defaults_off_and_ignores_the_master_guard() {
        use crate::core::preferences::PreferenceStore;
        use crate::core::prefs::{CODING_AGENT_AUTO_ANSWER, COMMAND_GUARD};
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let set = |key: &'static str, value: &'static str| {
            PreferenceStore::set_row_for_test(&pool, key, value)
        };
        assert!(!coding_agent_auto_answer(&pool).await, "off by default");
        set(COMMAND_GUARD.key(), "true").await.unwrap();
        assert!(
            !coding_agent_auto_answer(&pool).await,
            "the master guard does not turn it on"
        );
        set(COMMAND_GUARD.key(), "false").await.unwrap();
        set(CODING_AGENT_AUTO_ANSWER.key(), "true").await.unwrap();
        assert!(
            coding_agent_auto_answer(&pool).await,
            "on with the master guard off"
        );
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// End to end for Claude Code: a `Bash` card reaches the gate only under
    /// auto-answer, whatever the master guard says. Without a judge handle only
    /// the static half answers.
    #[tokio::test]
    async fn a_bash_card_is_classified_only_under_auto_answer() {
        use crate::core::preferences::PreferenceStore;
        use crate::core::prefs::{CODING_AGENT_AUTO_ANSWER, COMMAND_GUARD};
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let ask = || {
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "b".into(),
                    tool_name: "Bash".into(),
                    input: serde_json::json!({ "command": "ps aux | grep bench" }),
                },
            )
        };

        PreferenceStore::set_row_for_test(&pool, CODING_AGENT_AUTO_ANSWER.key(), "true")
            .await
            .unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), ask())
            .await
            .expect("a classified Bash card must not wait for a card");
        assert!(outcome.allowed, "auto-answer answers with the guard off");
        assert_eq!(outcome.reason.as_deref(), Some(COMMAND_GUARD_ALLOW_REASON));
        assert_eq!(count_permission_events(&pool, thread_id).await, (0, 0));

        PreferenceStore::set_row_for_test(&pool, CODING_AGENT_AUTO_ANSWER.key(), "false")
            .await
            .unwrap();
        PreferenceStore::set_row_for_test(&pool, COMMAND_GUARD.key(), "true")
            .await
            .unwrap();
        let waiting = tokio::time::timeout(std::time::Duration::from_millis(500), ask()).await;
        assert!(
            waiting.is_err(),
            "auto-answer off: the Bash card waits for the user, guard on or not"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A trigger-rooted session takes the same gates as any other, so the
    /// command gate decides this probe rather than a trigger's grant.
    #[tokio::test]
    async fn a_trigger_session_reaches_the_command_gate() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        insert_origin_event(
            &pool,
            thread_id,
            "TriggerStarted",
            &scheduler_origin("trig-order"),
        )
        .await;

        let outcome = prompt_coding_agent_permission(
            &pool,
            &bus,
            &pending,
            Path::new("/ws"),
            None,
            None,
            CodingAgentPermissionInput {
                thread_id,
                tool_use_id: "i".into(),
                tool_name: "command_execution".into(),
                input: zsh("ps aux"),
            },
        )
        .await;

        assert!(outcome.allowed);
        assert_eq!(outcome.reason.as_deref(), Some(COMMAND_GUARD_ALLOW_REASON));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The reported cards: a main check's fix thread and a lane thread never
    /// read the workspace's grants, so every refused shape parked a card.
    /// Whoever started the session, the workspace's own grant covers it.
    #[tokio::test]
    async fn every_session_uses_the_workspace_grants_without_a_card() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let workspace = workspace_granting(&["Bash"]);

        for origin in [
            device("owner-phone"),
            MessageOrigin::engine(EngineReason::MainCheckRed),
            MessageOrigin::engine(EngineReason::MergeConflict),
            scheduler_origin("trig-granted"),
        ] {
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;
            insert_origin_event(&pool, thread_id, "MessageReceived", &origin).await;

            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    workspace.path(),
                    None,
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "landing".to_string(),
                        tool_name: "Bash".to_string(),
                        input: serde_json::json!({
                            "command": "./scripts/harden-gates.sh 2>&1 | tail -30"
                        }),
                    },
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("{origin:?}: a granted request never waits for a card"));

            assert!(outcome.allowed, "{origin:?}");
            assert_eq!(
                outcome.reason.as_deref(),
                Some(PERSISTED_ALLOW_REASON),
                "{origin:?}"
            );
            assert_eq!(card_count(&pool, thread_id).await, 0, "{origin:?}");
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The recorded request is persisted AND fanned out over SSE, so it takes
    /// the same scrub a card applies.
    #[tokio::test]
    async fn an_auto_deny_redacts_the_command_it_records() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let pending = Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let secret = "postgres://u:hunter2@localhost:5432/db";

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt_coding_agent_permission(
                &pool,
                &bus,
                &pending,
                Path::new("/ws"),
                None,
                None,
                CodingAgentPermissionInput {
                    thread_id,
                    tool_use_id: "i".into(),
                    tool_name: "command_execution".into(),
                    input: serde_json::json!({
                        "command": format!("psql {secret} -c 'select 1' && rm -rf /")
                    }),
                },
            ),
        )
        .await
        .expect("must not hang");
        assert!(!outcome.allowed, "the catastrophic floor denies");

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
                    Path::new("/tmp"),
                    None,
                    None,
                    CodingAgentPermissionInput {
                        thread_id,
                        tool_use_id: "i1".to_string(),
                        tool_name: "command_execution".to_string(),
                        // A command the escalation classifier will not wave
                        // through, so this test still reaches the card it is
                        // about. `sudo ls` used to sit here and now auto-allows.
                        input: serde_json::json!({"command": "curl -X POST https://example.com -d @x"}),
                    },
                )
                .await
            })
        };

        // Wait for the canonical entry to register, then resolve it the way
        // the consent endpoint does. Bounded, because a request that stops
        // raising a card would otherwise spin here until the suite's timeout
        // rather than failing with a name.
        let mut request_id = None;
        for _ in 0..500 {
            request_id = {
                let state = pending.lock().unwrap();
                state.by_request_id.keys().next().cloned()
            };
            if request_id.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let request_id = request_id.expect("the request must raise a card within 5s");
        let entry = {
            let mut state = pending.lock().unwrap();
            state.take(&request_id).expect("canonical entry present")
        };
        assert_eq!(entry.tool_name, "command_execution");
        let _ = entry.tx.send(PermissionAnswer::Allowed);

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
        assert_eq!(
            rows[0].0["summary"],
            "command_execution curl -X POST https://example.com -d @x"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A prior "Allow for this thread" click must short-circuit the prompt:
    /// no event, immediate allow with the session-allow reason.
    #[tokio::test]
    async fn prompt_skips_card_on_session_allow_match() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let pending = std::sync::Arc::new(Mutex::new(PermissionState::default()));
        let thread_id = Uuid::new_v4();

        let input = serde_json::json!({"command": "git status"});
        let pattern = grant_offer("Bash", &input, &TrustedHeads::NONE)
            .session
            .expect("bash offers a session pattern");
        pending.lock().unwrap().allow_session(thread_id, pattern);

        let outcome = prompt_coding_agent_permission(
            &pool,
            &bus,
            &pending,
            Path::new("/tmp"),
            None,
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
                workspace.path(),
                None,
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
                Path::new("/ws"),
                Some(&f.root),
                None,
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
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    Path::new("/ws"),
                    Some(Path::new("/ws/.lucidos/worktrees/thread-abc")),
                    None,
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
        let _ = entry.tx.send(PermissionAnswer::Allowed);

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

    /// A card the engine withdraws is not a user decision. A new message or an
    /// idling session used to reach the blocked agent as "User denied", so it
    /// abandoned an approach nobody had rejected.
    #[tokio::test]
    async fn a_withdrawn_card_never_tells_the_agent_the_user_denied() {
        use crate::test_support::{setup_test_db, teardown_test_db};
        for (label, sweep_reason) in [
            ("superseded", SUPERSEDED_REASON),
            ("session ended", SESSION_ENDED_REASON),
        ] {
            let (pool, db_name) = setup_test_db().await;
            let (bus, _rx) = EventBus::new(pool.clone());
            let pending = Arc::new(Mutex::new(PermissionState::default()));
            let thread_id = Uuid::new_v4();
            seed_cc_thread(&bus, thread_id).await;

            let waiter = {
                let (pool, bus, pending) = (pool.clone(), bus.clone(), pending.clone());
                tokio::spawn(async move {
                    prompt_coding_agent_permission(
                        &pool,
                        &bus,
                        &pending,
                        Path::new("/ws"),
                        Some(Path::new("/ws/.lucidos/worktrees/thread-abc")),
                        None,
                        CodingAgentPermissionInput {
                            thread_id,
                            tool_use_id: "i-withdrawn".into(),
                            tool_name: "Write".into(),
                            input: serde_json::json!({ "file_path": "/home/u/notes.md" }),
                        },
                    )
                    .await
                })
            };
            await_canonical_request(&pending).await;
            // The request event lands after registration, so wait for it
            // before sweeping, as a real follow-up would.
            while card_count(&pool, thread_id).await == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }

            if sweep_reason == SUPERSEDED_REASON {
                resolve_pending_permissions_as_superseded(&pool, &bus, &pending, thread_id, None)
                    .await;
            } else {
                resolve_pending_permissions_as_session_ended(
                    &pool, &bus, &pending, thread_id, None,
                )
                .await;
            }

            let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
                .await
                .expect("resolves within 10s")
                .expect("task ok");
            assert!(!outcome.allowed, "{label}: a withdrawn card allows nothing");
            let reason = outcome.reason.unwrap_or_default();
            assert_ne!(
                reason, DENIAL_REASON,
                "{label}: nobody clicked Deny, so the agent must not read a user denial"
            );
            assert!(
                reason.starts_with(sweep_reason) && reason.contains("not a user decision"),
                "{label}: the agent reads why the card ended: {reason:?}"
            );

            pool.close().await;
            teardown_test_db(&db_name).await;
        }
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
                grant_offer: Some(grant_offer(tool_name, &input, &TrustedHeads::NONE)),
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
                Path::new("/ws"),
                None,
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

    /// Hydration restores the pattern the card stamped, not a re-derivation.
    /// A trusted script's path pattern needed trust to derive, and trust is
    /// not known at hydration time (ADR 0571).
    #[tokio::test]
    async fn hydration_restores_the_stamped_session_pattern() {
        use crate::engine::claude_code::AllowScope;
        use crate::test_support::{setup_test_db, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_cc_thread(&bus, thread_id).await;
        let stamped = "command_execution(./scripts/x.sh:*)";
        for event in [
            ThreadEvent::CodingAgentPermissionRequest {
                request_id: "req-path".to_string(),
                tool_use_id: "tu-path".to_string(),
                tool_name: CODEX_COMMAND_TOOL.to_string(),
                input: serde_json::json!({ "command": "/bin/zsh -lc ./scripts/x.sh", "cwd": "/ws" }),
                summary: "command_execution ./scripts/x.sh".to_string(),
                grant_offer: Some(GrantOffer {
                    session: Some(stamped.to_string()),
                    ..GrantOffer::default()
                }),
            },
            ThreadEvent::CodingAgentPermissionResolved {
                request_id: "req-path".to_string(),
                allowed: true,
                reason: None,
                persist_scope: Some(AllowScope::Session),
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

        let pending = Mutex::new(PermissionState::default());
        hydrate_session_allows(&pool, &pending, thread_id).await;
        {
            let state = pending.lock().unwrap();
            assert!(state.matches_session_allow(thread_id, stamped));
            assert_eq!(state.session_allows[&thread_id].len(), 1);
        }

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
            tokio::spawn(async move {
                prompt_coding_agent_permission(
                    &pool,
                    &bus,
                    &pending,
                    Path::new("/ws"),
                    None,
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
        let _ = entry.tx.send(PermissionAnswer::Allowed);

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
