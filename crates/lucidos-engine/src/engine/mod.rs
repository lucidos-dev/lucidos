pub(crate) mod agent_question;
pub mod agent_recovery;
pub(crate) mod agent_session;
mod agentic_loop;
pub mod cc_permission;
pub mod cc_question_wait;
pub(crate) mod cc_settings;
mod change_ops;
mod chat;
pub(crate) mod claude_code;
mod context;
mod document;
pub mod event_bus;
pub(crate) mod git_ops;
pub mod http;
mod memory;
pub mod memory_consumer;
mod pending_apply_actors;
pub mod thread_events;
pub mod thread_lifecycle;
pub mod thread_state;
pub(crate) mod tools;
pub mod types;
pub mod worktree_cleanup;

pub(crate) use chat::generate_thread_title;
#[cfg(test)]
pub(crate) use context::format_history_steps;
pub(crate) use context::validate_tool_use_pairing;
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
use crate::memory::{FastEmbedProvider, MemoryExtractor, PgVectorIndex};
use crate::runtime::{
    AgentKind, AgentRuntime, BrowserLogins, BrowserRuntime, ClaudeCodeRuntime, HeadlessBlocklist,
    PythonRuntime,
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
    /// Wake-only kick from a child completion. The loop loads the typed
    /// `ChildThreadCompleted` row identified by `child_completed_event_id`
    /// and projects it inline as the next user-channel block; no
    /// `UserPromptInjected` is persisted.
    WakeFromChild {
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
    },
}

/// Per-thread state: cancellation token + injection channel for mid-flight prompts.
pub struct ThreadHandle {
    pub token: CancellationToken,
    pub injection_tx: mpsc::UnboundedSender<InjectedPrompt>,
    /// Monotonic generation counter — incremented on each registration.
    /// Used by ThreadGuard::drop to avoid removing a newer registration.
    pub generation: u64,
}

/// Global counter for ThreadHandle generations.
static THREAD_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct LucidosEngine {
    artifact_manager: ArtifactManager,
    event_store: EventStore,
    python_runtime: PythonRuntime,
    browser_runtime: BrowserRuntime,
    app_manager: Arc<AppManager>,
    llm: Arc<dyn LlmProvider>,
    embedder: Arc<FastEmbedProvider>,
    memory_index: Option<PgVectorIndex>,
    extractor: Option<MemoryExtractor>,
    /// Vertex project ID — used to build image providers on demand.
    vertex_project_id: String,
    /// Shared region handle, updated in place when `vertex_region` changes.
    vertex_location: crate::llm::vertex::LocationHandle,
    vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
    openai_api_key: Option<String>,
    rebuilding_memory: AtomicBool,
    cancel_rebuild: AtomicBool,
    /// Acquired via `scheduler::BackupGuard::try_acquire`. POST /api/backup
    /// returns 409 when the guard is held; the scheduled cron skips its tick.
    pub backup_in_progress: AtomicBool,
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
    /// Engine-shipped reference knowhow (`<repo_root>/system-knowhow/`).
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
            std::collections::HashMap<
                String,
                Arc<crate::api::proxy_wasm_signer::CompiledModule>,
            >,
        >,
    >,
    /// Shared wasmtime engine. Module compilation + per-request
    /// instantiation must use the SAME engine (wasmtime forbids
    /// cross-engine instantiation), so we hold it on the engine and hand
    /// it out via `wasm_engine()`.
    wasm_engine: Arc<wasmtime::Engine>,
    /// Pending App UI capture requests. Key: request_id, Value: oneshot sender.
    /// Tool handlers insert a sender, the /api/app-capture endpoint resolves it.
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
    /// Active Claude Code sessions keyed by thread_id.
    pub(crate) agent_sessions: Arc<tokio::sync::Mutex<HashMap<Uuid, AgentSession>>>,
    /// Registered coding-agent backends (Claude Code, Codex, …).
    /// Engine code spawns agents via this registry instead of naming a concrete runtime.
    pub(crate) agent_runtimes: HashMap<AgentKind, Arc<dyn AgentRuntime>>,
    /// Per-thread timestamps of the last CC session spawn — used to debounce duplicate requests.
    /// Keyed by thread_id so concurrent starts on different threads are not blocked.
    last_cc_spawn: std::sync::Mutex<HashMap<Uuid, std::time::Instant>>,
    /// Per-thread spawn-coalescer. Phase 2 made every CC subprocess exit on
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
    /// Pending MCP consent requests. Key: request_id, Value: oneshot sender (true=allow, false=deny).
    pub pending_mcp_consent: Arc<
        std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    >,
    /// Pending CC permission prompts, deduped by `(thread, tool, input)` so
    /// CC's parallel/repeat tool calls collapse onto one card. See
    /// `cc_permission` module docs.
    pub pending_cc_permission: Arc<std::sync::Mutex<cc_permission::CcPermissionState>>,
    /// Rendezvous map for the AskUserQuestion PreToolUse hook — the hook's
    /// long-poll handler waits on a receiver here; `answer_pending_question`
    /// notifies it when the user picks an answer. See `cc_question_wait` docs.
    pub question_wait_registry: cc_question_wait::QuestionWaitRegistry,
    /// Per-`change_id` stash for the actor of an in-flight Apply that hands
    /// the merge off to a fresh CC subprocess (Tier 3 slow path / conflict
    /// resolution). The cleanup in `agent_session::run_session` takes the
    /// actor back out so `ChangeApplied` / `ChangeApplyFailed` carry the
    /// device that clicked Apply instead of collapsing to "Lucidos Engine".
    pub(crate) pending_apply_actors: pending_apply_actors::PendingApplyActors,
    /// Weak self-reference for spawning background tasks that need Arc<Self>
    self_arc: std::sync::OnceLock<std::sync::Weak<LucidosEngine>>,
    /// EventBus — single emission point for all domain events.
    /// Producers call typed methods, consumers subscribe to the broadcast channel.
    pub event_bus: event_bus::EventBus,
    /// CC commands cache keyed by repo root — each repo has different tools.
    /// Populated from CC Init events, persisted to `.lucidos/cc-commands.json`.
    pub(crate) cc_commands_cache: tokio::sync::RwLock<HashMap<String, CcCommandsInfo>>,
    /// Shared in-memory trigger configs — same Arc as SchedulerManager's.
    /// Allows engine tools to read trigger state without going through the scheduler.
    pub(crate) trigger_configs:
        Arc<std::sync::RwLock<HashMap<String, crate::triggers::TriggerConfig>>>,
    /// Live tasks for the `run_bash_background` chat tool. Holds only
    /// currently-running tasks; finished tasks are persisted to the
    /// events stream as `BackgroundBashCompleted` and evicted. See
    /// `tools/bash_background.rs`.
    pub(crate) bash_background: tools::bash_background::BackgroundBashRegistry,
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
}

fn spawn_vertex_region_subscriber(
    mut rx: tokio::sync::broadcast::Receiver<event_bus::EmittedEvent>,
    location: crate::llm::vertex::LocationHandle,
) {
    tokio::spawn(async move {
        use event_bus::{BusEvent, SystemEvent};
        while let Ok(emitted) = rx.recv().await {
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
                    log!("[Preferences] vertex_region updated live: {} → {}", old, new_region);
                }
                Err(e) => log!("[Preferences] vertex_region update skipped (lock poisoned): {}", e),
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


/// `processing_thread_ids - all_cc_thread_ids`. Idle CC sessions stay in
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
/// see a misleading "Canceled". CC sessions also get `external_terminal_emitted`
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
            Some(EventChannel::CodingAgent)
        } else {
            None
        }
    };

    let request_event_id = crate::engine::agent_session::latest_originating_event_id(
        pool,
        thread_id,
        &[
            "MessageReceived",
            "CodingAgentUserMessageSent",
            "TriggerStarted",
        ],
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
#[path = "mod_tests.rs"]
mod tests;
