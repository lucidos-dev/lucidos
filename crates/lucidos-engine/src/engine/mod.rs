pub(crate) mod agent_context;
pub(crate) mod agent_question;
pub mod agent_recovery;
pub(crate) mod agent_session;
mod agentic_loop;
mod apply_all_batches;
pub(crate) mod apply_all_driver;
pub mod cc_permission;
pub mod cc_question_wait;
pub(crate) mod cc_settings;
mod change_ops;
mod chat;
pub(crate) mod claude_code;
pub(crate) mod command_guard;
pub(crate) mod command_judge;
pub mod command_permission;
mod context;
pub mod engine_version;
pub mod event_bus;
mod frontend_refresh;
pub(crate) mod git_ops;
pub mod http;
pub(crate) mod inline_question_repair;
pub(crate) mod inline_tool_call_repair;
pub(crate) mod loaded_knowhow;
pub mod mcp_permission;
mod memory;
pub mod memory_consumer;
mod pending_apply_actors;
pub(crate) mod preferences;
mod session_seed;
pub mod startup_lease;
pub mod supervisor_respawn_sidecar;
pub mod thread_events;
pub mod thread_lifecycle;
pub mod thread_queue;
pub mod thread_state;
pub mod todo_consumer;
pub(crate) mod tools;
pub(crate) mod trigger_group_writes;
pub mod types;
pub mod worktree_cleanup;

pub(crate) use agentic_loop::{
    coalesced_images_for_reprocess, coalesced_user_text_for_reprocess,
    emit_user_prompt_injected_event, filter_removed_queued_prompts, strip_app_capture_marker,
};
pub(crate) use change_ops::now_epoch_millis;
pub(crate) use chat::generate_thread_title;
pub(crate) use chat::PreEmittedOrigin;
#[cfg(test)]
pub(crate) use context::format_history_steps;
pub use types::*;

/// Public re-export so binaries (notably `main.rs`) can start the CC spawn
/// dispatcher at engine startup. The rest of `agent_session` stays
/// crate-private — only this background task entry point is public.
pub mod spawn_dispatcher {
    pub use super::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};
}

use crate::core::{
    AppManager, ArtifactManager, CredentialStore, EventStore, PinnedAppStore, PreferenceStore,
    PREF_MODEL_TITLE,
};
use crate::llm::LlmProvider;
use crate::memory::{EmbedderSlot, MemoryExtractor, PgVectorIndex};
use crate::runtime::{
    AgentRuntime, BrowserLogins, BrowserRuntime, ClaudeCodeRuntime, CodexRuntime, CodingAgent,
    HeadlessBlocklist, PythonRuntime,
};
use git_ops::{auto_commit_safe_files_if_dirty, git_cmd};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A mid-flight prompt injected into the agentic loop.
/// Also used as `OrphanedInjection` (type alias) for injections that arrived
/// after the loop exited but before the ThreadGuard dropped.
#[derive(Clone, Debug)]
pub struct InjectedPrompt {
    pub text: String,
    /// Client-provided UUID for the injecting message. Carried through the
    /// channel for callers that need correlation, but NOT reused as the
    /// persisted `UserPromptInjected.id` — the chat fast-path emits a
    /// `MessageReceived` row with this UUID first, so reusing it would
    /// collide on `events_pkey`. Frontend reconciles pending messages via
    /// `MessageReceived.id`; the UPI link back to the request is carried
    /// by `EventMeta::request_event_id`.
    pub event_id: Option<Uuid>,
    /// Semantic mode of the actor that generated this injection — Human (user
    /// typed), Agent (parent thread's LLM), or Engine (recovery / scheduler).
    pub mode: thread_events::ActorMode,
    /// Event in the parent thread that triggered this injection (mode != Human).
    pub spawning_event_id: Option<Uuid>,
    /// Optional images attached to the injected message.
    pub images: Option<Vec<crate::api::ChatImage>>,
    /// Structured origin describing the actor (e.g. ThreadLink::Child for a
    /// child→parent callback). Stamped onto the persisted UserPromptInjected
    /// event so the chip can render the right initiator label.
    pub origin: Option<thread_events::MessageOrigin>,
    pub kind: InjectedPromptKind,
}

#[derive(Clone, Debug)]
pub enum InjectedPromptKind {
    /// Synthesised user message — emits `UserPromptInjected` and pushes the
    /// framed text into the next agentic-loop turn.
    UserText,
    /// Child-completion wake. The parent's `ChildThreadCompleted` event is
    /// already the exchange-starter on the wire (caller passed its id as
    /// `prompt.spawning_event_id`); the loop projects `prompt.text` inline
    /// as the next user-channel block WITHOUT emitting `UserPromptInjected`
    /// — otherwise the response would split into a duplicate exchange and
    /// strand the rich child-completion card.
    WakeFromChild,
}

/// Per-thread state: cancellation token + injection channel for mid-flight prompts.
pub struct ThreadHandle {
    pub token: CancellationToken,
    /// Private so every send from outside this module goes through
    /// [`ThreadHandle::inject`], which also counts and wakes. A bare `.send()`
    /// would enqueue the user's message without waking a tool parked waiting
    /// for it. (The module's own tests do drive the raw channel — they're
    /// exercising the channel plumbing itself, a level below `inject`.)
    injection_tx: mpsc::UnboundedSender<InjectedPrompt>,
    /// Fires whenever a prompt is injected into this thread. The agentic
    /// loop picks injections up with `try_recv` *between* iterations, so a
    /// tool that blocks — `bash_output(wait_secs=120)` is the one that
    /// really can — would otherwise sit on its full budget while the user's
    /// follow-up waits. Blocking tools select on this and come back early.
    /// `notify_waiters` is right here (not `notify_one`): a stored permit
    /// would make the NEXT wait return instantly for an injection the loop
    /// has already consumed.
    ///
    /// A notification alone is not enough — see [`Self::pending_injections`].
    pub injection_notify: Arc<tokio::sync::Notify>,
    /// Prompts delivered to `injection_tx` that the loop has not drained yet.
    ///
    /// `notify_waiters` reaches only waiters that are *already* registered, so
    /// on its own it covers the narrow "injected during the wait" case and
    /// misses the wide one: a message that arrives while the LLM call is in
    /// flight sits in the channel until the next iteration's `try_recv`, and a
    /// blocking tool started in *this* iteration would see no notification at
    /// all and sit out its whole budget. A blocking tool therefore registers
    /// its waiter first and then reads this counter, so an injection either
    /// shows up here or wakes the registered waiter — never neither.
    pub pending_injections: Arc<std::sync::atomic::AtomicUsize>,
    /// Monotonic generation counter — incremented on each registration.
    /// Used by ThreadGuard::drop to avoid removing a newer registration.
    pub generation: u64,
    /// Set by `cancel_thread` so the agentic-loop cancel arm can stamp the
    /// emitted `ResponseCanceled` with the actor that clicked Stop. Drained
    /// once via `take_cancel_actor` to avoid reusing a stale device across
    /// requests. The `CancellationToken` itself remains signal-only.
    pub cancel_actor: Arc<std::sync::Mutex<Option<thread_events::MessageOrigin>>>,
}

impl ThreadHandle {
    pub fn new(
        token: CancellationToken,
        injection_tx: mpsc::UnboundedSender<InjectedPrompt>,
        generation: u64,
    ) -> Self {
        ThreadHandle {
            token,
            injection_tx,
            injection_notify: Arc::new(tokio::sync::Notify::new()),
            pending_injections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            generation,
            cancel_actor: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Deliver a prompt to the running agentic loop and wake anything
    /// blocking on this thread. Returns false when the receiver is gone
    /// (the turn ended between the caller's lookup and this send) — the
    /// caller then falls back to starting a fresh turn.
    ///
    /// Count BEFORE sending, and wake after. Both orderings are load-bearing:
    ///
    /// - **Count before send.** `send` publishes the prompt to the loop at
    ///   once, so a drain can report it consumed before a post-send increment
    ///   lands. The saturating decrement would then no-op against a zero
    ///   count and the late `+1` would strand a phantom unread forever —
    ///   every later `bash_output(wait_secs=…)` on the thread would refuse to
    ///   block, which is the polling storm all of this exists to stop.
    /// - **Wake after count.** A blocking tool registers its waiter and then
    ///   reads the counter, so it must never see "no notification AND no
    ///   pending work".
    pub fn inject(&self, prompt: InjectedPrompt) -> bool {
        self.pending_injections
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        if self.injection_tx.send(prompt).is_ok() {
            self.injection_notify.notify_waiters();
            true
        } else {
            // Nothing was delivered, and no drain can be racing us — the
            // receiver is gone. Give the reservation back.
            self.injections_drained(1);
            false
        }
    }

    /// Record that the agentic loop took `n` prompts off the channel.
    /// Saturating: the counter tracks the channel, and an underflow would wrap
    /// to `usize::MAX` and stop every later wait from blocking, forever.
    pub fn injections_drained(&self, n: usize) {
        if n == 0 {
            return;
        }
        // Infallible by construction — `fetch_update` only returns `Err` when
        // the closure returns `None`, and this one always returns `Some`.
        let _ = self.pending_injections.fetch_update(
            std::sync::atomic::Ordering::Release,
            std::sync::atomic::Ordering::Acquire,
            |cur| Some(cur.saturating_sub(n)),
        );
    }
}

/// Global counter for ThreadHandle generations.
static THREAD_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct LucidosEngine {
    artifact_manager: ArtifactManager,
    event_store: EventStore,
    python_runtime: PythonRuntime,
    browser_runtime: BrowserRuntime,
    app_manager: Arc<AppManager>,
    /// Active LLM provider behind a swappable handle so the credential subscriber
    /// (`spawn_provider_credential_subscriber`) can hot-swap it at runtime when a
    /// provider credential is added/removed — no engine restart. All reads go
    /// through [`LucidosEngine::current_provider`], which clones the inner `Arc`
    /// out under a short read guard (never held across an `.await`). Mirrors the
    /// `Arc<RwLock<…>>` convention of `ModelRegistry` / `LocationHandle`.
    llm: Arc<std::sync::RwLock<Arc<dyn LlmProvider>>>,
    /// Backends for the `web_search` tool, in preference order. Held behind the
    /// same swappable handle as `llm` and rebuilt by the same credential
    /// subscriber, so adding a provider key enables search without a restart.
    ///
    /// Deliberately NOT derived from the chat model's provider: search resolves
    /// over the whole configured provider set, which is what lets a user on a
    /// provider with no search tool (OpenRouter, a local endpoint) still search
    /// via another configured one. See `llm::web_search`.
    web_search: Arc<std::sync::RwLock<Arc<crate::llm::WebSearchChain>>>,
    /// Late-binding embedder slot: boots EMPTY (so boot never waits on the
    /// multi-hundred-MB model), and the background loader
    /// (`spawn_embedder_load`) installs the model without a restart once it
    /// lands. Until then memory features degrade descriptively. See
    /// `memory::EmbedderSlot`.
    embedder: Arc<EmbedderSlot>,
    memory_index: Option<PgVectorIndex>,
    extractor: Option<MemoryExtractor>,
    /// Vertex project ID — used to build image providers on demand.
    vertex_project_id: String,
    /// Shared region handle, updated in place when `vertex_region` changes.
    vertex_location: crate::llm::vertex::LocationHandle,
    vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
    /// Shared model routing map (provider + declared context window), reloaded
    /// in place by `spawn_models_registry_subscriber` on any `Model*` event.
    /// The engine holds it — not just `RoutingProvider` — because the context
    /// trimmer needs the declared context window to size its budget.
    model_registry: crate::llm::model_registry::ModelRegistry,
    openai_api_key: Option<String>,
    rebuilding_memory: AtomicBool,
    cancel_rebuild: AtomicBool,
    /// Set once the engine has begun graceful shutdown or a restart (`main.rs`
    /// signal handler and `abort_in_flight_for_restart`). The scheduler's event
    /// subscriber reads it to stop firing event-triggers: the terminator events
    /// emitted during cleanup (`ResponseAborted{EngineShutdown}`,
    /// `CodingAgentIdled`, `SessionEnded`) otherwise fan out to triggers whose
    /// scripts call back into the HTTP API being torn down — `lucidos ...` gets
    /// connection-refused and the script dies, surfacing a spurious
    /// "<trigger> failed" push. Never reset; the process is on its way out.
    shutting_down: AtomicBool,
    /// Acquired via `scheduler::BackupGuard::try_acquire`. POST /api/v1/backup
    /// returns 409 when the guard is held; the scheduled cron skips its tick.
    pub backup_in_progress: AtomicBool,
    /// Dev-only background-rebuild state driving the "new version available"
    /// surface. Set by the Apply-triggered rebuild (Phase 2); read by
    /// `GET /api/v1/engine/version-status`. Idle in packaged (no source rebuild).
    /// See `engine/engine_version.rs`.
    build_state: std::sync::RwLock<engine_version::BuildState>,
    /// Memoized "is a newer engine binary on disk?" verdict, keyed by the running
    /// binary's last-seen mtime, so a polling client doesn't fork
    /// `current_exe --build-id` every tick (mirrors the gateway's `UpdateCheck`).
    update_check: std::sync::Mutex<engine_version::UpdateCheck>,
    /// Throttled cache of "is the engine SOURCE behind HEAD with a
    /// restart-requiring change pending?" (the `engine_source_matches_head` git
    /// check). Read by `GET /api/v1/engine/version-status` (polled every ~4s per
    /// client) and the self-heal driver, so the underlying `git diff` runs at most
    /// once per TTL regardless of client count. Dev-only; always false packaged.
    /// See `engine_version::source_behind_head`.
    source_behind_cache: std::sync::Mutex<engine_version::SourceBehindCache>,
    /// Memoized "is the on-disk binary's commit an ANCESTOR of the running
    /// engine's?" verdict, keyed by the on-disk build id. Answers the direction
    /// question `update_available` needs — a DIFFERENT binary is only an update
    /// when it isn't an older one — without forking `git merge-base` on every
    /// ~4s version-status poll. Dev-only. See
    /// `engine_version::disk_binary_is_upgrade`.
    disk_direction_cache: std::sync::Mutex<engine_version::DiskDirectionCache>,
    /// Self-heal bookkeeping: how many background rebuilds this engine has
    /// auto-triggered for the current HEAD, so a genuinely broken `main` can't
    /// spin builds forever (bounded per HEAD; reset when HEAD moves). Dev-only.
    /// See `engine_version::self_heal_engine_version_if_needed`.
    self_heal_state: std::sync::Mutex<engine_version::SelfHealState>,
    /// Handle to the in-flight Apply-triggered background rebuild task (dev), so a
    /// later Apply can coalesce — abort the running build and start over. The
    /// build child is `kill_on_drop`, so aborting the task kills the cargo process.
    /// See `engine_version::trigger_background_rebuild`.
    build_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Monotonic generation so only the latest background rebuild updates
    /// `build_state` — a superseded build's completion is ignored.
    build_generation: std::sync::atomic::AtomicU64,
    /// Dev-only: swappable served-frontend dir. `api::serve_frontend` reads the
    /// current snapshot path per request; a *frontend-only* Apply re-snapshots
    /// `dist/` and swaps this so the served client advances WITHOUT an engine
    /// respawn (INV-A: only when the engine binary is unchanged — a mixed change
    /// still advances only via a Switch). `None` in packaged / headless (no
    /// `LUCIDOS_STATIC_DIR`). Set once by `api::create_router` via
    /// `init_served_frontend`. See `engine::frontend_refresh`.
    served_frontend: std::sync::OnceLock<Arc<std::sync::RwLock<PathBuf>>>,
    /// The source dir (`LUCIDOS_STATIC_DIR` = live `dist/`) that served-frontend
    /// snapshots are taken from. Set alongside `served_frontend`.
    served_frontend_source: std::sync::OnceLock<PathBuf>,
    /// Monotonic generation for served-frontend re-snapshots: coalesces rapid
    /// frontend-only Applies (only the latest generation swaps) AND names the
    /// snapshot subdir. Boot pins generation 0; the first refresh is generation 1.
    /// Mirrors `build_generation`.
    frontend_refresh_generation: std::sync::atomic::AtomicU64,
    /// Handle to the in-flight served-frontend refresh task, so a later Apply can
    /// abort + supersede it. Mirrors `build_task`.
    frontend_refresh_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Has the worktree-pinned-frontend warning already fired this process? The
    /// check runs on the ~10s peer-sync tick, so without this it would log every
    /// tick forever. A field rather than a `static` so tests that build several
    /// engines each get their own latch. See
    /// `engine::frontend_refresh::warn_once_if_frontend_worktree_pinned`.
    frontend_worktree_pin_warned: std::sync::atomic::AtomicBool,
    /// Device actor stashed by the switch handler (`/api/v1/restart`) at
    /// request time and read by the graceful-shutdown boundary emit at ACTUAL
    /// teardown — the HTTP handler has the device, the SIGUSR1 signal handler
    /// does not. Present → a user-initiated switch (attributes "You" / enables
    /// auto-resume on recovery); absent → a non-switch stop (System attribution,
    /// manual Continue). `take`n once at teardown so a later non-switch stop
    /// can't reuse a stale actor. See `engine_version` + Phase 3/4 of the plan.
    restart_actor: std::sync::Mutex<Option<thread_events::MessageOrigin>>,
    /// Thread ids `recover_orphaned_worktrees` decided to auto-resume after a
    /// user-initiated *Switch to new version* (in-flight coding-agent threads with
    /// a device-attributed teardown boundary). Drained by `main.rs` AFTER the spawn
    /// dispatcher subscribes — recovery runs before it, so emitting
    /// `ContinuationRequested` during recovery would be missed. A crash-interrupted
    /// thread is NOT enqueued here (it keeps the manual Continue affordance) — the
    /// loop-safety guarantee.
    pending_switch_resumes: std::sync::Mutex<Vec<Uuid>>,
    /// Per-thread handles (cancellation token + injection channel). Key = thread_id.
    /// Uses std::sync::Mutex since operations are trivial (insert/remove),
    /// and this allows the ThreadGuard to clean up synchronously in Drop (even on panic).
    active_threads: Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    /// Per-thread completion notifiers for queuing follow-up requests.
    /// When a thread finishes (guard drops), it notifies waiters so queued
    /// requests can proceed instead of cancelling in-progress work.
    thread_completion: Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    workspace_path: PathBuf,
    /// Lucidos source repo root (resolved at startup via `git_ops::main_worktree`).
    /// Stored so the API surface can fall back to a known path when the
    /// `repositories` row is missing (e.g. e2e tests truncate the table).
    repo_root: PathBuf,
    /// User-level Lucidos directory (~/.lucidos), git-tracked for shared knowhow
    user_dir: Option<PathBuf>,
    /// Engine-shipped reference knowhow (the staged `LUCIDOS_SYSTEM_KNOWHOW_DIR`
    /// on packaged builds, `<repo_root>/system-knowhow/` on a dev checkout —
    /// see `core::system_knowhow::resolve_system_knowhow_dir`).
    /// Read-only; never overrideable by a workspace's local knowhow.
    system_knowhow_dir: Option<PathBuf>,
    /// User profile - always included in context for broad queries
    user_profile: tokio::sync::RwLock<String>,
    /// User's timezone (IANA format, e.g., "America/New_York")
    user_timezone: tokio::sync::RwLock<String>,
    /// User's preferred language (e.g., "English", "Spanish")
    user_language: tokio::sync::RwLock<String>,
    /// Database pool for credentials and preferences
    pool: sqlx::PgPool,
    /// In-memory cache of `script_handshake` proxy auth headers, shared
    /// across both the HTTP proxy and the `proxy_request` LLM tool so the
    /// handshake script runs once per expiry window regardless of caller.
    proxy_token_cache: Arc<crate::api::proxy_token_cache::ProxyTokenCache>,
    /// Compiled WASM signer modules (`data/auth-modules/<name>.wasm`)
    /// loaded at startup. Keyed by file basename (without `.wasm`). Wrapped
    /// in `RwLock` so the Phase-9 reload endpoint can swap the map atomically.
    proxy_modules: Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, Arc<crate::api::proxy_wasm_signer::CompiledModule>>,
        >,
    >,
    /// Shared wasmtime engine. Module compilation + per-request
    /// instantiation must use the SAME engine (wasmtime forbids
    /// cross-engine instantiation), so we hold it on the engine and hand
    /// it out via `wasm_engine()`.
    wasm_engine: Arc<wasmtime::Engine>,
    /// Pending App UI capture requests. Key: request_id, Value: oneshot sender.
    /// Tool handlers insert a sender, the /api/v1/app-capture endpoint resolves it.
    pub pending_captures: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, tokio::sync::oneshot::Sender<CaptureResult>>,
        >,
    >,
    /// Plugin installs awaiting user confirmation in the install panel.
    /// Key: install_id (UUID). Value: staged plugin tree + manifest. Inserted
    /// when `install_plugin` returns the `[PLUGIN_INSTALL_REQUEST]` sentinel;
    /// removed when the user clicks Confirm or Cancel in the panel (or when
    /// the engine restarts — the staged temp dir is dropped on shutdown).
    /// Allowed-ephemeral per CLAUDE.md (same justification as
    /// `pending_captures`).
    pub pending_installs: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, crate::engine::tools::plugins::PendingInstall>,
        >,
    >,
    /// Pending plugin uninstalls awaiting confirm/cancel from the uninstall
    /// panel. Mirrors `pending_installs` exactly — entry registered when
    /// `uninstall_plugin` returns the `[PLUGIN_UNINSTALL_REQUEST]` sentinel,
    /// removed on Confirm/Cancel (or engine restart). Allowed-ephemeral.
    pub pending_uninstalls: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, crate::engine::tools::plugins::PendingUninstall>,
        >,
    >,
    /// Frontend origin URL (e.g., "https://lucidos.example.com"), set from first request's Origin header
    pub frontend_origin: std::sync::Mutex<Option<String>>,
    /// Active coding-agent sessions keyed by thread_id.
    pub(crate) agent_sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    /// Per-thread loaded knowhow set — populated when `load_knowhow` is called,
    /// consumed when assembling the user message + stubbing resume tool blocks.
    /// Reconstructable from `ToolResult` events on engine restart (task 2.3).
    pub(crate) loaded_knowhow: Arc<crate::engine::loaded_knowhow::LoadedKnowhowStore>,
    /// Registered coding-agent backends (Claude Code, Codex, …).
    /// Engine code spawns agents via this registry instead of naming a concrete runtime.
    pub(crate) agent_runtimes: HashMap<CodingAgent, Arc<dyn AgentRuntime>>,
    /// Per-thread timestamps of the last Claude Code session spawn — used to debounce duplicate requests.
    /// Keyed by thread_id so concurrent starts on different threads are not blocked.
    last_cc_spawn: std::sync::Mutex<HashMap<Uuid, std::time::Instant>>,
    /// Pre-spawn map of `cc_thread_id` → `app_id` for app coding-agent
    /// threads. `spawn_agent_thread` stashes the app id here before
    /// `process_message_with_steps` runs; `run_direct_agent` pops it in to
    /// dispatch sparse-checkout worktree creation. Cleared when the first
    /// `SessionStarted` event lands (the value is then persisted on the
    /// event payload and in `thread_summaries.coding_agent_kind`).
    pub(crate) pending_app_spawn: std::sync::Mutex<HashMap<Uuid, String>>,
    /// Per-thread spawn-coalescer. Phase 2 made every Claude Code subprocess exit on
    /// idle, so two rapid follow-ups (within ~250ms) used to either race two
    /// subprocesses or drop the second message with a "duplicate request"
    /// error. The coalescer elects one leader per thread; followers within
    /// the debounce window queue their messages for the leader to drain into
    /// a single combined CC input. Phase 5's event-driven dispatcher will
    /// subsume this. See `agent_session/cc_spawn_coalesce.rs`.
    pub(crate) cc_spawn_coalesce: agent_session::CcSpawnCoalescer,
    /// Limits concurrent CC process startups to prevent CPU contention.
    /// Acquired before spawn_or_resume(), released after Init event.
    cc_startup_semaphore: Arc<tokio::sync::Semaphore>,
    /// Backs `lock_workspace_repo()` — serializes mutations to the workspace
    /// repo's working tree against `change_ops::apply_change`'s dirty check
    /// so apply never observes a half-written file from a commit-in-flight.
    workspace_repo_lock: Arc<tokio::sync::Mutex<()>>,
    /// MCP server manager — handles lifecycle, tool discovery, and tool calls
    pub mcp_manager: crate::mcp::McpManager,
    /// Pending CC permission prompts, deduped by `(thread, tool, input)` so
    /// CC's parallel/repeat tool calls collapse onto one card. See
    /// `cc_permission` module docs.
    pub pending_cc_permission: Arc<std::sync::Mutex<cc_permission::PermissionState>>,
    /// Pending command-guard permission prompts (ADR 0002) — the chat mirror of
    /// `pending_cc_permission`, using the same dedup / session-allow mechanism.
    /// The chat agent's loop blocks in-process on the entry's broadcast rather
    /// than over MCP. See `command_permission` + `command_guard`.
    pub pending_command_permission: Arc<std::sync::Mutex<cc_permission::PermissionState>>,
    /// Pending MCP permission prompts (chat) — the chat mirror of
    /// `pending_command_permission` for MCP server tool calls, using the same
    /// dedup / session-allow mechanism. The chat agent's loop blocks in-process
    /// on the entry's broadcast. See `mcp_permission`.
    pub pending_mcp_permission: Arc<std::sync::Mutex<cc_permission::PermissionState>>,
    /// Rendezvous map for the AskUserQuestion PreToolUse hook — the hook's
    /// long-poll handler waits on a receiver here; `answer_pending_question`
    /// notifies it when the user picks an answer. See `cc_question_wait` docs.
    pub question_wait_registry: cc_question_wait::QuestionWaitRegistry,
    /// Per-`change_id` stash for the actor of an in-flight Apply that hands
    /// the merge off to a fresh Claude Code subprocess (Tier 3 slow path / conflict
    /// resolution). The cleanup in `agent_session::run_session` takes the
    /// actor back out so `ChangeApplied` / `ChangeApplyFailed` carry the
    /// device that clicked Apply instead of collapsing to "Lucidos Engine".
    pub(crate) pending_apply_actors: pending_apply_actors::PendingApplyActors,
    /// In-flight Apply All batches (see `apply_all_batches`). Each entry is
    /// the live state of one batch — what's been applied, what's failed,
    /// what's still pending. The driver advances the batch from inside
    /// `emit_change_applied` / `emit_apply_failed` so the conflict-recovery
    /// suspension+resume happens organically: the recovery CC's eventual
    /// `ChangeApplied` for the conflict member triggers the next apply
    /// via the same hook the happy path uses.
    pub(crate) apply_all_batches: Arc<tokio::sync::Mutex<apply_all_batches::ApplyAllRegistry>>,
    /// Sender for the apply-all driver task. `emit_change_applied` /
    /// `emit_apply_failed` push `Applied` / `Failed` messages here; the
    /// driver task (spawned at engine startup via `start_apply_all_driver`)
    /// is the only consumer. The channel decouples the recursive
    /// `apply_change → emit_change_applied → driver → apply_change` cycle —
    /// without it the futures form an async recursion the compiler can't
    /// auto-trait-check for Send.
    pub(crate) apply_all_drive_tx:
        tokio::sync::mpsc::UnboundedSender<apply_all_driver::ApplyAllDriveMsg>,
    /// Weak self-reference for spawning background tasks that need Arc<Self>
    self_arc: std::sync::OnceLock<std::sync::Weak<LucidosEngine>>,
    /// EventBus — single emission point for all domain events.
    /// Producers call typed methods, consumers subscribe to the broadcast channel.
    pub event_bus: event_bus::EventBus,
    /// In-memory pong inbox for the PresenceCheck protocol — owned here so
    /// both the API handler (`POST /api/v1/presence-pong`) and the fan-out
    /// code (`send_push_to_all_with_app`) share the same tracker.
    /// See `system-knowhow/notifications.md` §3. Transient by design;
    /// cleared on engine restart (no recovery is meaningful — in-flight
    /// pongs from a previous process are stale).
    pub presence_tracker: crate::api::presence_pong::PresenceTracker,
    /// Live count of open SSE connections (`GET /api/v1/events`). The push
    /// fan-out gates the PresenceCheck on this — a connected page can pong
    /// even when its `device_presence` heartbeat has gone stale (iOS suspends
    /// the 30s timer while the PWA is foregrounded). See
    /// `system-knowhow/notifications.md` §3. Transient — reset on restart.
    pub sse_connections: crate::api::sse_connections::SseConnectionCounter,
    /// CC commands cache keyed by repo root — each repo has different tools.
    /// Populated from CC Init events, persisted to `.lucidos/cc-commands.json`.
    pub(crate) cc_commands_cache: tokio::sync::RwLock<HashMap<String, CcCommandsInfo>>,
    /// Shared in-memory trigger configs — same Arc as SchedulerManager's.
    /// Allows engine tools to read trigger state without going through the scheduler.
    pub(crate) trigger_configs:
        Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>>,
    /// Shared in-memory trigger groups — user-visible folders shown in the
    /// triggers panel. Same Arc-sharing pattern as `trigger_configs`. Groups
    /// don't schedule anything; the SchedulerManager just owns the loader for
    /// startup-replay symmetry with triggers, while HTTP / LLM tool callers
    /// read this registry directly.
    pub(crate) trigger_groups:
        Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerGroup>>>,
    /// Serializes invariant-bearing writes (create, rename) on
    /// `trigger_groups`. The std `RwLock` on the registry above can't be held
    /// across `.await`, which leaves a TOCTOU window between the read-time
    /// case-insensitive dedup check and the projection apply — two parallel
    /// POSTs with the same name can both pass dedup and both insert. This
    /// async mutex closes the window: helpers in `trigger_group_writes`
    /// acquire it for the full read-dedup + emit + apply span, so the
    /// unique-name invariant holds even under concurrent requests. Reads,
    /// delete, and reorder don't acquire it (they can't violate the
    /// invariant; delete-then-create false-positive 409 is a separate,
    /// known-quirk).
    pub(crate) trigger_group_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Live tasks for the `run_bash_background` chat tool. Holds only
    /// currently-running tasks; finished tasks are persisted to the
    /// events stream as `BackgroundBashCompleted` and evicted. See
    /// `tools/bash_background.rs`.
    pub(crate) bash_background: tools::bash_background::BackgroundBashRegistry,
    /// The *Thread Queue* — system-wide admission control for background
    /// spawns (event-trigger fires, cron fires, agent-driven sub-thread /
    /// coding-agent spawns). User-initiated chat never routes through it.
    /// In-memory queue/active state mirrors the `thread_queue` projection
    /// and is rebuilt from it at boot (`recover_persisted_entries`).
    pub thread_queue: Arc<thread_queue::ThreadQueue>,
}

/// RAII guard that removes a thread from active_threads when dropped.
/// This ensures cleanup happens even if the processing task panics.
/// Also notifies any queued requests waiting for this thread to finish.
pub struct ThreadGuard {
    active_threads: Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
    thread_id: Uuid,
    completion_notify: Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
    /// Generation when this guard was created. Drop only removes the
    /// active_threads entry if the generation still matches — prevents a
    /// force-evicted guard from removing a newer registration.
    generation: u64,
}

impl ThreadGuard {
    /// The registration this guard owns. Anything that reaches back into
    /// `active_threads` on behalf of *this* turn must check it, or it will act
    /// on a newer registration that replaced this one — see
    /// [`LucidosEngine::note_injections_drained`] and `Drop` below.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Drop for ThreadGuard {
    fn drop(&mut self) {
        let owned = if let Ok(mut threads) = self.active_threads.lock() {
            // Only remove if the generation matches — a force-evicted guard must
            // not remove a newer registration for the same thread_id.
            if threads
                .get(&self.thread_id)
                .is_some_and(|h| h.generation == self.generation)
            {
                threads.remove(&self.thread_id);
                true
            } else {
                false
            }
        } else {
            false
        };
        // Only notify completion waiters if we owned this thread
        if owned {
            if let Ok(mut completions) = self.completion_notify.lock() {
                if let Some(notify) = completions.remove(&self.thread_id) {
                    notify.notify_waiters();
                }
            }
        }
    }
}

// Thread-local to pass parent_callback_rx from EventBus::new() (inside LucidosEngine::new)
// to start_parent_callback_listener() (called after Arc::new(engine)).
thread_local! {
    static PARENT_CALLBACK_RX: std::cell::RefCell<Option<tokio::sync::mpsc::UnboundedReceiver<event_bus::ParentCallback>>> = const { std::cell::RefCell::new(None) };
    /// Apply-All driver receiver — same pattern as PARENT_CALLBACK_RX. The
    /// channel is created inside `LucidosEngine::new`; the tx is stored on
    /// the engine struct, and the rx is stashed here so
    /// `start_apply_all_driver` (called after `Arc::new(engine)`) can pick
    /// it up.
    static APPLY_ALL_DRIVE_RX: std::cell::RefCell<Option<tokio::sync::mpsc::UnboundedReceiver<apply_all_driver::ApplyAllDriveMsg>>> = const { std::cell::RefCell::new(None) };
}

fn spawn_vertex_region_subscriber(
    mut rx: tokio::sync::broadcast::Receiver<event_bus::EmittedEvent>,
    location: crate::llm::vertex::LocationHandle,
) {
    tokio::spawn(async move {
        use event_bus::{BusEvent, SystemEvent};
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let emitted = match rx.recv().await {
                Ok(e) => e,
                // Lag = subscriber fell behind. Skip the dropped events but
                // keep listening; without this the loop would exit on a
                // single lag and stop tracking vertex_region changes for the
                // rest of the engine's lifetime.
                Err(RecvError::Lagged(n)) => {
                    log!(
                        "[Preferences] vertex_region subscriber lagged by {} events — continuing",
                        n
                    );
                    continue;
                }
                Err(RecvError::Closed) => break,
            };
            let BusEvent::System(SystemEvent::PreferencesChanged { key, value, .. }) =
                &emitted.typed
            else {
                continue;
            };
            if key != crate::core::PREF_VERTEX_REGION {
                continue;
            }
            let Some(new_region) = value else { continue };
            match location.write() {
                Ok(mut guard) => {
                    let old = std::mem::replace(&mut *guard, new_region.clone());
                    log!(
                        "[Preferences] vertex_region updated live: {} → {}",
                        old,
                        new_region
                    );
                }
                Err(e) => log!(
                    "[Preferences] vertex_region update skipped (lock poisoned): {}",
                    e
                ),
            }
        }
    });
}

/// Keep the in-memory model→provider [`ModelRegistry`] in sync with the `models`
/// table. On any `Model{Created,Updated,Deleted}` event the whole table is
/// re-queried and the map swapped wholesale — the table is tiny, and a wholesale
/// swap avoids incremental-update drift. Mirrors `spawn_vertex_region_subscriber`.
fn spawn_models_registry_subscriber(
    mut rx: tokio::sync::broadcast::Receiver<event_bus::EmittedEvent>,
    registry: crate::llm::model_registry::ModelRegistry,
    pool: sqlx::PgPool,
) {
    tokio::spawn(async move {
        use event_bus::{BusEvent, SystemEvent};
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let emitted = match rx.recv().await {
                Ok(e) => e,
                // Lag = subscriber fell behind. Skip the dropped events but keep
                // listening, so a single lag doesn't stop tracking model changes
                // for the rest of the engine's lifetime.
                Err(RecvError::Lagged(n)) => {
                    log!(
                        "[ModelRegistry] registry subscriber lagged by {} events — continuing",
                        n
                    );
                    continue;
                }
                Err(RecvError::Closed) => break,
            };
            if !matches!(
                &emitted.typed,
                BusEvent::System(
                    SystemEvent::ModelCreated { .. }
                        | SystemEvent::ModelUpdated { .. }
                        | SystemEvent::ModelDeleted { .. }
                )
            ) {
                continue;
            }
            let fresh = crate::llm::model_registry::load_from_db(&pool).await;
            match registry.write() {
                Ok(mut guard) => {
                    *guard = fresh;
                    log!(
                        "[ModelRegistry] reloaded model→provider map ({} entries)",
                        guard.len()
                    );
                }
                Err(e) => log!("[ModelRegistry] reload skipped (lock poisoned): {}", e),
            }
        }
    });
}

/// Hot-swap the engine's active LLM provider when a provider credential changes.
/// On any `Credential{Created,Updated,Deleted}` for a provider service
/// (`openai`/`anthropic`/`openrouter`/`local`), re-resolve `select_provider`
/// against current DB state via [`crate::llm::build_active_provider`] and swap
/// the shared provider handle in place — so a first-run user who adds a key in
/// Settings → Models → Providers gets a working chat with NO restart, and removing the
/// last key swaps back to the unconfigured sentinel (when
/// `LUCIDOS_BOOT_WITHOUT_PROVIDER` is set). Mirrors
/// `spawn_models_registry_subscriber`.
///
/// **Mock isolation:** the caller does not spawn this under `LUCIDOS_MODEL=mock`,
/// and `ctx.model_is_mock` is `false`, so `build_active_provider` can never
/// return `MockProvider` here — the mock stays reachable only via the explicit
/// env opt-in. A `FailFast` rebuild (no provider, gate off) keeps the current
/// provider rather than panicking the running engine.
fn spawn_provider_credential_subscriber(
    mut rx: tokio::sync::broadcast::Receiver<event_bus::EmittedEvent>,
    llm_handle: Arc<std::sync::RwLock<Arc<dyn crate::llm::LlmProvider>>>,
    web_search_handle: Arc<std::sync::RwLock<Arc<crate::llm::WebSearchChain>>>,
    pool: sqlx::PgPool,
    ctx: crate::llm::ProviderBuildContext,
) {
    tokio::spawn(async move {
        use event_bus::{BusEvent, SystemEvent};
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let emitted = match rx.recv().await {
                Ok(e) => e,
                // Lag = subscriber fell behind. Skip the dropped events but keep
                // listening, so a single lag doesn't stop tracking credential
                // changes for the rest of the engine's lifetime.
                Err(RecvError::Lagged(n)) => {
                    log!(
                        "[Providers] credential subscriber lagged by {} events — continuing",
                        n
                    );
                    continue;
                }
                Err(RecvError::Closed) => break,
            };
            let service = match &emitted.typed {
                BusEvent::System(
                    SystemEvent::CredentialCreated { service_name, .. }
                    | SystemEvent::CredentialUpdated { service_name, .. }
                    | SystemEvent::CredentialDeleted { service_name, .. },
                ) => service_name,
                _ => continue,
            };
            if !crate::llm::PROVIDER_CREDENTIAL_SERVICES.contains(&service.as_str()) {
                continue;
            }
            match crate::llm::build_active_provider(Some(&pool), &ctx).await {
                Ok(crate::llm::ProviderBuildOutcome::Install {
                    llm,
                    web_search,
                    selection,
                }) => {
                    match llm_handle.write() {
                        Ok(mut guard) => {
                            *guard = llm;
                            log!(
                                "[Providers] active LLM provider swapped to {:?} after '{}' credential change — no restart",
                                selection,
                                service
                            );
                        }
                        Err(e) => {
                            log!("[Providers] provider swap skipped (lock poisoned): {}", e)
                        }
                    }
                    // Swapped in the same pass as the LLM provider: adding an
                    // Anthropic or OpenAI key must enable web_search without a
                    // restart, exactly as it enables chat.
                    match web_search_handle.write() {
                        Ok(mut guard) => {
                            let ids = web_search.backend_ids();
                            *guard = web_search;
                            log!(
                                "[Providers] web search backends now: {}",
                                if ids.is_empty() {
                                    "none configured".to_string()
                                } else {
                                    ids.join(" → ")
                                }
                            );
                        }
                        Err(e) => {
                            log!("[Providers] web search swap skipped (lock poisoned): {}", e)
                        }
                    }
                }
                Ok(crate::llm::ProviderBuildOutcome::FailFast) => {
                    log!(
                        "[Providers] '{}' credential change left no provider configured and LUCIDOS_BOOT_WITHOUT_PROVIDER is off — keeping the current provider (a restart would fail-fast)",
                        service
                    );
                }
                Err(e) => {
                    log!(
                        "[Providers] provider rebuild after '{}' credential change failed: {} — keeping current provider",
                        service,
                        e
                    );
                }
            }
        }
    });
}

/// One-shot migration: WASM auth signers used to live under
/// `data/artifacts/auth-modules/`; they now live at `data/auth-modules/` so
/// plugin bundles can ship them under their own top-level `auth-modules/`
/// directory mirroring the new path. Renames the legacy dir if and only if
/// the new path is absent — never merges, never clobbers. Filesystem errors
/// are logged but non-fatal: the load step that follows surfaces a stale
/// path as an empty module map.
pub(crate) fn migrate_legacy_auth_modules_dir(workspace_path: &Path) {
    let legacy = workspace_path.join("data/artifacts/auth-modules");
    let new_dir = workspace_path.join("data/auth-modules");
    if !legacy.is_dir() {
        return;
    }
    if new_dir.exists() {
        log!(
            "[Startup] both legacy {} and new {} auth-modules dirs exist; \
             leaving both alone (engine reads new path only)",
            legacy.display(),
            new_dir.display()
        );
        return;
    }
    if let Some(parent) = new_dir.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log!(
                "[Startup] could not create parent {} for auth-modules migration: {}",
                parent.display(),
                e
            );
            return;
        }
    }
    match std::fs::rename(&legacy, &new_dir) {
        Ok(()) => log!(
            "[Startup] migrated auth-modules dir: {} -> {}",
            legacy.display(),
            new_dir.display()
        ),
        Err(e) => log!(
            "[Startup] auth-modules migration failed ({} -> {}): {}",
            legacy.display(),
            new_dir.display(),
            e
        ),
    }
}

mod engine_impl;

/// `processing_thread_ids - all_cc_thread_ids`. Idle coding-agent sessions stay in
/// `active_threads` between turns, so the exclusion set must cover them or
/// they get misclassified as chat threads.
fn partition_chat_thread_ids(
    processing_thread_ids: &[Uuid],
    all_cc_thread_ids: &std::collections::HashSet<Uuid>,
) -> Vec<Uuid> {
    processing_thread_ids
        .iter()
        .filter(|tid| !all_cc_thread_ids.contains(tid))
        .copied()
        .collect()
}

/// Emit a `ResponseAborted` (actor=System) for a thread the engine is
/// force-evicting after the `register_thread_queued` 60s timeout. Without
/// this pre-emit, the run-loop's stop arm would default to `ResponseCanceled`
/// (`is_shutdown=false`, no user-action suppress flag set) and the user would
/// see a misleading "Canceled". Coding-agent sessions also get `external_terminal_emitted`
/// set so the run-loop arm skips its duplicate emit; chat threads' `agentic_loop`
/// may still emit a duplicate `ResponseCanceled`, which the frontend deflates
/// because `Aborted` is checked before `Canceled` in `exchangeStatus`.
pub(crate) async fn emit_stuck_thread_eviction_abort(
    bus: &event_bus::EventBus,
    pool: &sqlx::PgPool,
    agent_sessions: &tokio::sync::Mutex<HashMap<Uuid, types::AgentSession>>,
    thread_id: Uuid,
) {
    use thread_events::{EventChannel, EventMeta, MessageOrigin};

    let channel = {
        let guard = agent_sessions.lock().await;
        if let Some(s) = guard.get(&thread_id) {
            s.external_terminal_emitted
                .store(true, std::sync::atomic::Ordering::Release);
            Some(EventChannel::ClaudeCode)
        } else {
            None
        }
    };

    // CC threads anchor on `MessageReceived` / `CodingAgentUserMessageSent` /
    // `TriggerStarted` / `ChildThreadCompleted` (any can start a CC turn —
    // CCUMS for live follow-ups, CTC for parents waking from a finished
    // child via `notify_parent_of_child_completion`). Chat threads use the
    // same list minus CCUMS. The shared constants live in
    // `agent_session::resume`.
    let originating_types: &[&str] = if channel == Some(EventChannel::ClaudeCode) {
        crate::engine::agent_session::CC_ORIGINATING_EVENT_TYPES
    } else {
        crate::engine::agent_session::CHAT_ORIGINATING_EVENT_TYPES
    };
    let request_event_id = crate::engine::agent_session::latest_originating_event_id(
        pool,
        thread_id,
        originating_types,
    )
    .await;

    thread_events::emit_response_aborted(
        bus,
        thread_id,
        thread_events::AbortCause::SafetyNet,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            channel,
            request_event_id,
            actor: Some(MessageOrigin::system()),
            ..EventMeta::NONE
        },
        "[Engine] ResponseAborted (stuck-thread eviction)",
    )
    .await;
}

#[cfg(test)]
#[path = "mod_tests/common.rs"]
mod common;

#[cfg(test)]
#[path = "mod_tests/migration.rs"]
mod migration_tests;

#[cfg(test)]
#[path = "mod_tests/lifecycle.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "mod_tests/injection.rs"]
mod injection_tests;
