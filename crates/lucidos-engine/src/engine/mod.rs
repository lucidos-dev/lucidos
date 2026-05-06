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
use std::path::PathBuf;
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
    /// Pending App UI capture requests. Key: request_id, Value: oneshot sender.
    /// Tool handlers insert a sender, the /api/app-capture endpoint resolves it.
    pub pending_captures: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, tokio::sync::oneshot::Sender<CaptureResult>>,
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

impl LucidosEngine {
    pub(crate) const DEFAULT_REPO_NAME: &'static str = "Lucidos";

    /// Start the parent callback listener. Must be called after Arc::new(engine).
    /// Takes the mpsc receiver that was stashed during construction and spawns
    /// a task that resumes parent threads when child threads complete.
    pub fn start_parent_callback_listener(self: &Arc<Self>) {
        let rx = PARENT_CALLBACK_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            crate::log!("[FanOut] No parent callback receiver found — listener not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(cb) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    crate::log!(
                        "[FanOut] Child {} completed, resuming parent {}",
                        cb.child_thread_id,
                        cb.parent_thread_id
                    );
                    // Without explicit child-link origin the synthesizer can't
                    // attribute this message — chip falls back to "Lucidos Engine".
                    let callback_origin =
                        Some(crate::engine::thread_events::MessageOrigin::thread_link_child(
                            cb.child_thread_id,
                            thread_events::ActorMode::Agent,
                        ));

                    // If the parent thread is already running (processing another child's
                    // callback), inject into its agentic loop instead of going through
                    // register_thread_queued — which would wait 60s then force-cancel
                    // the parent's in-progress response, causing a false "Canceled" label.
                    if engine.inject_prompt(
                        cb.parent_thread_id,
                        cb.callback_text.clone(),
                        None,
                        thread_events::ActorMode::Agent,
                        None,
                        callback_origin.clone(),
                    ) {
                        crate::log!(
                            "[FanOut] Injected child {} callback into active parent {}",
                            cb.child_thread_id,
                            cb.parent_thread_id
                        );
                        return;
                    }
                    // Parent is idle — start a new request normally.
                    if let Err(e) = engine
                        .process_message_with_steps(
                            &cb.callback_text,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some(cb.parent_thread_id),
                            None,
                            None,
                            None,
                            None,
                            None,
                            crate::engine::thread_events::ActorMode::Agent,
                            None,
                            None,
                            None,
                            callback_origin,
                        )
                        .await
                    {
                        crate::log!(
                            "[FanOut] Failed to resume parent thread {}: {}",
                            cb.parent_thread_id,
                            e
                        );
                    }
                });
            }
        });
    }

    /// Drain the spawn dispatcher's outbound channel and actuate each
    /// [`SpawnRequest`] against `run_direct_agent`. Started once at engine
    /// boot, after `SpawnDispatcher::spawn(..)` returns the receiver end.
    ///
    /// Today only `SpawnRequest::Continue` flows through this channel — the
    /// chat HTTP handler still owns the spawn for `MessageReceived` (see
    /// `spawn_dispatcher` module docs). For a continue request we re-enter
    /// `run_direct_agent` with empty input so CC reconnects via `--resume`
    /// against the existing session id; CC then sees its prior interrupt
    /// state and continues from there. Phase 5.3 owns enriching this with
    /// the real recovery payload (worktree path, branch, system prompt).
    pub fn start_spawn_request_consumer(
        self: &Arc<Self>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<
            crate::engine::spawn_dispatcher::SpawnRequest,
        >,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move {
                    use crate::engine::spawn_dispatcher::SpawnRequest;
                    match req {
                        SpawnRequest::Continue {
                            thread_id,
                            event_id,
                        } => {
                            crate::log!(
                                "[SpawnConsumer] Continue thread={} event={} — invoking run_direct_agent (--resume + non-empty stdin placeholder)",
                                thread_id,
                                event_id
                            );

                            // Load the originating ContinueSignal's actor +
                            // reason. The actor stamps the resume boundary so
                            // it reads "You restarted" rather than "⚙ System".
                            // The reason gates the boundary itself: only the
                            // user-clicked-continue path opens a "Resumed
                            // after engine restart" exchange — an
                            // `answered_after_idle` continuation belongs
                            // inside the existing AskUserQuestion exchange.
                            let (continue_actor, continue_reason): (
                                Option<crate::engine::thread_events::MessageOrigin>,
                                Option<String>,
                            ) = match engine.event_store().get_event_by_id(event_id).await {
                                Ok(Some(row)) => {
                                    let actor = row.payload.get("actor").and_then(|v| {
                                        match serde_json::from_value::<
                                            crate::engine::thread_events::MessageOrigin,
                                        >(v.clone())
                                        {
                                            Ok(o) => Some(o),
                                            Err(e) => {
                                                crate::log!(
                                                    "[SpawnConsumer] Continue event {} has malformed actor payload: {} — chip will fall back to engine",
                                                    event_id, e
                                                );
                                                None
                                            }
                                        }
                                    });
                                    let reason = row
                                        .payload
                                        .get("reason")
                                        .and_then(|v| v.as_str())
                                        .map(String::from);
                                    (actor, reason)
                                }
                                Ok(None) => (None, None),
                                Err(e) => {
                                    crate::log!(
                                        "[SpawnConsumer] Failed to load continue event {}: {} — chip will fall back to engine and the resume boundary will be skipped",
                                        event_id, e
                                    );
                                    (None, None)
                                }
                            };

                            // SessionRecovered opens the resume exchange in
                            // the timeline before any CC text streams. CC's
                            // own `--resume` system carries the prior context
                            // to the model, so no engine note (unlike the
                            // chat/rerun path). Skipped for non-recovery
                            // continuations (answered_after_idle) so the
                            // follow-up CC events fold into the existing
                            // AskUserQuestion exchange instead of being
                            // mislabeled "Resumed after engine restart".
                            if crate::engine::agent_recovery::continue_should_open_resume_exchange(
                                continue_reason.as_deref(),
                            ) {
                                engine
                                    .event_bus
                                    .emit_or_log(
                                        crate::engine::event_bus::BusEvent::Thread {
                                            thread_id,
                                            event: crate::engine::thread_events::ThreadEvent::SessionRecovered {
                                                branch: String::new(),
                                                origin: None,
                                            },
                                            meta: crate::engine::thread_events::EventMeta {
                                                channel: Some(
                                                    crate::engine::thread_events::EventChannel::CodingAgent,
                                                ),
                                                actor: continue_actor,
                                                ..crate::engine::thread_events::EventMeta::NONE
                                            },
                                        },
                                        "[SpawnConsumer] SessionRecovered (continue)",
                                    )
                                    .await;
                            }

                            // Resolve the latest CC session id from the events
                            // table so `--resume` lands on the prior conversation.
                            let resume_sid =
                                crate::engine::agent_session::lookup_latest_cc_session_id(
                                    engine.pool(),
                                    thread_id,
                                )
                                .await;
                            let request_id = uuid::Uuid::new_v4();
                            let cancel_token =
                                tokio_util::sync::CancellationToken::new();
                            // Must stay non-empty — see CONTINUE_RESUME_USER_MESSAGE
                            // doc for the empty-stdin zombie write-up.
                            let result = engine
                                .run_direct_agent(
                                    request_id,
                                    thread_id,
                                    crate::engine::agent_recovery::CONTINUE_RESUME_USER_MESSAGE,
                                    None,
                                    event_id,
                                    None,
                                    &cancel_token,
                                    None,
                                    None,
                                    None,
                                    None,
                                    resume_sid,
                                    None,
                                    None,
                                    None,
                                )
                                .await;
                            if let Err(e) = result {
                                crate::log!(
                                    "[SpawnConsumer] Continue thread={} event={} failed: {}",
                                    thread_id,
                                    event_id,
                                    e
                                );
                            }
                        }
                    }
                });
            }
            crate::log!("[SpawnConsumer] channel closed — consumer exiting");
        });
    }

    pub async fn new(
        workspace_path: PathBuf,
        database_url: &str,
        llm: Arc<dyn LlmProvider>,
        vertex_project_id: String,
        vertex_location: crate::llm::vertex::LocationHandle,
        vertex_token_cache: Option<crate::llm::vertex::TokenCache>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let artifact_manager = ArtifactManager::new(workspace_path.clone())?;

        // Ensure .gitignore exists for ephemeral directories
        let gitignore_path = workspace_path.join(".gitignore");
        if !gitignore_path.exists() {
            if let Err(e) = std::fs::write(&gitignore_path, ".lucidos/\ndata/postgres/\n") {
                log!("[Startup] Failed to write workspace .gitignore: {}", e);
            }
        }

        // Migrate legacy prompts/ directories to intents/ (idempotent)
        crate::core::migrate_prompts_to_intents(&workspace_path);

        // Single shared connection pool for the entire engine
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(50)
            .acquire_timeout(std::time::Duration::from_secs(60))
            .idle_timeout(std::time::Duration::from_secs(600))
            .max_lifetime(std::time::Duration::from_secs(1800))
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // Tune HNSW search: higher ef_search = better recall at slight latency cost
                    sqlx::query("SET hnsw.ef_search = 100")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url)
            .await?;

        // Run sqlx migrations before any schema init calls
        sqlx::migrate!().run(&pool).await?;

        let event_store = EventStore::new(pool.clone());
        event_store.init_schema().await?;

        // One-shot, idempotent: the SQL backfill in migration
        // 20260429214800 only read `payload->>'trigger_id'` and missed legacy
        // `task_id` events, leaving `thread_summaries.trigger_id` NULL on every
        // pre-rename trigger thread. Recover from the events table first…
        let from_events = event_store.backfill_trigger_id_from_events().await?;
        if from_events > 0 {
            log!(
                "[Engine] Backfilled {} thread_summaries rows: trigger_id NULL → first TriggerStarted event",
                from_events
            );
        }

        // …then translate any v5 hashes the pre-fix scheduler stamped into
        // `trigger_id` back to the raw `config.id` so the dropdown filter matches.
        // Subsequent runs update zero rows.
        let backfilled = event_store.backfill_trigger_id_v5_to_config_id().await?;
        if backfilled > 0 {
            log!(
                "[Engine] Backfilled {} thread_summaries rows: trigger_id v5-hash → config.id",
                backfilled
            );
        }
        let python_runtime = PythonRuntime::new(workspace_path.clone());
        let app_manager = Arc::new(AppManager::new(&workspace_path)?);

        let embedder = Arc::new(FastEmbedProvider::new()?);

        let browser_runtime = BrowserRuntime::new(workspace_path.clone(), pool.clone());

        // Initialize scheduler schemas (notifications)
        // This ensures tables exist even before SchedulerManager is created
        crate::scheduler::NotificationStore::init_schema(&pool).await?;

        // Initialize credentials, preferences, and pinned apps schemas
        CredentialStore::init_schema(&pool).await?;
        PreferenceStore::init_schema(&pool).await?;
        PinnedAppStore::init_schema(&pool).await?;
        HeadlessBlocklist::init_schema(&pool).await?;
        BrowserLogins::init_schema(&pool).await?;

        let openai_api_key = std::env::var("OPENAI_API_KEY").ok();

        let extractor = if vertex_project_id.is_empty() {
            log!("[Memory] No Vertex project configured — memory extraction disabled");
            None
        } else {
            let cache = vertex_token_cache
                .clone()
                .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(None)));
            Some(MemoryExtractor::with_location_handle(
                vertex_project_id.clone(),
                vertex_location.clone(),
                cache,
            ))
        };

        // Memory index uses PostgreSQL + pgvector for vector search
        let memory_index = match PgVectorIndex::new(pool.clone()).await {
            Ok(index) => {
                let count = index.len().await.unwrap_or(0);
                log!("[Memory] Index initialized (pgvector, {} entries)", count);
                Some(index)
            }
            Err(e) => {
                log!(
                    "[Memory] Could not initialize index: {}. Memory search disabled.",
                    e
                );
                None
            }
        };

        // Gated on memory_index because it owns the schema; without it
        // memory_entries may not exist.
        if let Some(index) = memory_index.clone() {
            let embedder = embedder.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::memory::reembed::reembed_stale(&index, embedder).await {
                    log!(@Memory, "Re-embed task failed: {}", e);
                }
            });
        }

        // Load user profile from artifacts, or use empty if doesn't exist
        let profile_path = workspace_path
            .join(crate::core::ARTIFACTS_DIR)
            .join("user_profile.md");
        let user_profile = std::fs::read_to_string(&profile_path).unwrap_or_default();
        if !user_profile.is_empty() {
            log!("[Memory] Loaded user profile ({} chars)", user_profile.len());
        }

        // Load user timezone from database, environment, or leave empty (LLM will ask)
        let user_timezone = match PreferenceStore::get(&pool, "timezone").await {
            Ok(Some(tz)) => tz,
            _ => String::new(),
        };

        if user_timezone.is_empty() {
            log!("[Engine] User timezone: not set (LLM will ask)");
        } else {
            log!("[Engine] User timezone: {}", user_timezone);
        }

        // Load user language preference from database
        let user_language = match PreferenceStore::get(&pool, "language").await {
            Ok(Some(lang)) => lang,
            _ => String::new(),
        };

        if user_language.is_empty() {
            log!("[Engine] User language: not set (will detect from conversation)");
        } else {
            log!("[Engine] User language: {}", user_language);
        }

        let repo_root = git_ops::main_worktree().await;

        let system_knowhow_dir = {
            let candidate = repo_root.join("system-knowhow");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        };

        // Register the Lucidos repo so it appears in the Files view without manual setup
        if let Err(e) = crate::core::repositories::RepositoryStore::ensure_exists(
            &pool,
            Self::DEFAULT_REPO_NAME,
            &repo_root.to_string_lossy(),
        )
        .await
        {
            log!(
                "[Startup] Failed to register default Lucidos repository: {}",
                e
            );
        }

        // Auto-commit safe files (docs) if dirty — a dirty working tree blocks
        // change apply/revert which require a clean state.
        auto_commit_safe_files_if_dirty(&repo_root).await;

        // Orphaned Claude Code worktrees (in-flight sessions interrupted by crash)
        // are recovered after engine creation by calling recover_orphaned_worktrees().
        // Idle sessions stay idle — they're shown in the WAITING UI on reload.

        // Construct EventBus early so startup reconciliation can emit events through it.
        let (event_bus, parent_callback_rx) = event_bus::EventBus::new(pool.clone());

        // Must run before list_pending()-based reconciliation below, so any
        // recovered pending row is visible to the stale-branch discard.
        match event_bus.changes_projection().rebuild_missing_from_events().await {
            Ok(0) => {}
            Ok(n) => log!("[Startup] Rebuilt {} missing change row(s) from events", n),
            Err(e) => log!("[Startup] rebuild_missing_from_events failed: {}", e),
        }

        // Clean up stale merge worktrees left by crashed apply_change operations.
        // The merge stays in the worktree; the change stays pending so the user can retry.
        let stale_merges = event_bus.changes_projection().with_merge_worktree().await;
        for change in stale_merges {
            if let (Some(wt), Some(tb)) = (&change.merge_worktree_path, &change.merge_temp_branch) {
                let change_repo = std::path::PathBuf::from(&change.repo_root);
                // git_cmd cleanup is best-effort — the worktree/branch may already be gone.
                let _ = git_cmd(&["merge", "--abort"], std::path::Path::new(wt)).await;
                let _ = git_cmd(&["worktree", "remove", "--force", wt], &change_repo).await;
                let _ = git_cmd(&["branch", "-D", tb], &change_repo).await;
                if let Some(tid) = change.thread_id {
                    event_bus
                        .emit_or_log(
                            event_bus::BusEvent::Thread {
                                thread_id: tid,
                                event: thread_events::ThreadEvent::MergeResolutionCleared {
                                    change_id: change.id.to_string(),
                                },
                                meta: thread_events::EventMeta::NONE,
                            },
                            "[Startup] MergeResolutionCleared",
                        )
                        .await;
                }
                log!(
                    "[Startup] Cleaned up stale merge worktree for change {}",
                    change.id
                );
            }
        }

        // Surface — but do NOT discard — pending changes whose branches no
        // longer exist. The user resolves these from Review explicitly; the
        // engine never auto-discards on the user's behalf. Apply on a
        // missing-branch row will fail with a useful error and the user
        // can then click Discard.
        let stale = event_bus.changes_projection().list_pending().await;
        for change in stale {
            let branch_ok = git_cmd(&["rev-parse", "--verify", &change.branch_name], &repo_root)
                .await
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !branch_ok {
                log!(
                    "[Engine] Pending change {} references missing branch {} — left in Review for user to resolve",
                    change.id,
                    change.branch_name
                );
            }
        }

        let mcp_manager = crate::mcp::McpManager::new(pool.clone());

        let user_dir = std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".lucidos"));

        if let Some(ref ud) = user_dir {
            crate::core::user_dir::ensure_git_init(ud);
        }

        spawn_vertex_region_subscriber(event_bus.subscribe(), vertex_location.clone());

        Ok(Self {
            artifact_manager,
            event_store,
            python_runtime,
            browser_runtime,
            app_manager,
            llm,
            embedder,
            memory_index,
            extractor,
            vertex_project_id,
            vertex_location,
            vertex_token_cache,
            openai_api_key,
            rebuilding_memory: AtomicBool::new(false),
            cancel_rebuild: AtomicBool::new(false),
            backup_in_progress: AtomicBool::new(false),
            active_threads: Arc::new(std::sync::Mutex::new(HashMap::new())),
            thread_completion: Arc::new(std::sync::Mutex::new(HashMap::new())),
            cc_commands_cache: tokio::sync::RwLock::new(Self::load_cc_commands_cache(
                &workspace_path,
            )),
            workspace_path,
            user_dir,
            system_knowhow_dir,
            user_profile: tokio::sync::RwLock::new(user_profile),
            user_timezone: tokio::sync::RwLock::new(user_timezone),
            user_language: tokio::sync::RwLock::new(user_language),
            event_bus: {
                // parent_callback_rx is stored temporarily in a thread-local;
                // it's extracted and wired up after Arc::new(engine) via start_parent_callback_listener().
                PARENT_CALLBACK_RX.with(|cell| cell.borrow_mut().replace(parent_callback_rx));
                event_bus
            },
            pool,
            pending_captures: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            frontend_origin: std::sync::Mutex::new(None),
            agent_sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            agent_runtimes: {
                let mut m: HashMap<AgentKind, Arc<dyn AgentRuntime>> = HashMap::new();
                m.insert(AgentKind::ClaudeCode, Arc::new(ClaudeCodeRuntime));
                m
            },
            last_cc_spawn: std::sync::Mutex::new(HashMap::new()),
            cc_spawn_coalesce: agent_session::CcSpawnCoalescer::new(),
            cc_startup_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            mcp_manager,
            pending_mcp_consent: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            pending_cc_permission: Arc::new(std::sync::Mutex::new(
                cc_permission::CcPermissionState::default(),
            )),
            question_wait_registry: cc_question_wait::QuestionWaitRegistry::new(),
            pending_apply_actors: pending_apply_actors::PendingApplyActors::default(),
            self_arc: std::sync::OnceLock::new(),
            trigger_configs: Arc::new(std::sync::RwLock::new(HashMap::new())),
        })
    }

    /// Load CC commands cache from `.lucidos/cc-commands.json` (survives engine restarts).
    fn load_cc_commands_cache(workspace: &std::path::Path) -> HashMap<String, CcCommandsInfo> {
        let path = workspace.join(".lucidos/cc-commands.json");
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                log!("[ClaudeCode] Failed to parse CC commands cache: {}", e);
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        }
    }

    /// Update a per-repo entry in cc_commands_cache and persist to disk if changed.
    pub(crate) async fn upsert_cc_commands_cache(&self, repo_key: String, info: CcCommandsInfo) {
        let json = {
            let mut cache = self.cc_commands_cache.write().await;
            if cache.get(&repo_key) == Some(&info) {
                return;
            }
            cache.insert(repo_key, info);
            match serde_json::to_string(&*cache) {
                Ok(json) => Some(json),
                Err(e) => {
                    log!("[ClaudeCode] Failed to serialize CC commands cache: {}", e);
                    None
                }
            }
        };
        if let Some(json) = json {
            let path = self.workspace_path.join(".lucidos/cc-commands.json");
            if let Err(e) = std::fs::write(&path, &json) {
                log!("[ClaudeCode] Failed to write CC commands cache: {}", e);
            }
        }
    }

    /// Resolve a data-relative path, returning both the normalized data-relative path and
    /// the absolute filesystem path. Paths without a known prefix are assumed to be under artifacts/.
    fn resolve_data_path(
        &self,
        relative_path: &str,
    ) -> Result<(String, std::path::PathBuf), String> {
        if crate::api::is_path_traversal(relative_path) {
            return Err("Path traversal not allowed".to_string());
        }
        // Strip leading "data/" if the LLM included the full workspace-relative path
        let relative_path = relative_path.strip_prefix("data/").unwrap_or(relative_path);
        let known_prefixes = [
            "artifacts/",
            "apps/",
            "knowhow/",
            "triggers/",
            "system-knowhow/",
        ];
        let normalized = if known_prefixes.iter().any(|p| relative_path.starts_with(p)) {
            relative_path.to_string()
        } else {
            format!("artifacts/{}", relative_path)
        };

        // System knowhow lives in the engine repo, not the workspace.
        if let Some(rel) = normalized.strip_prefix("system-knowhow/") {
            let dir = self
                .system_knowhow_dir
                .as_deref()
                .ok_or_else(|| "System knowhow is not available".to_string())?;
            return Ok((normalized.clone(), dir.join(rel)));
        }

        let full_path = self.workspace_path.join("data").join(&normalized);

        // For knowhow paths: if file doesn't exist locally, fall back to shared.
        if normalized.starts_with("knowhow/") && !full_path.exists() {
            let kh_relative = normalized.strip_prefix("knowhow/").unwrap();
            if let Some(shared_dir) = self.shared_knowhow_dir() {
                let shared_path = shared_dir.join(kh_relative);
                if shared_path.exists() {
                    return Ok((normalized, shared_path));
                }
            }
        }

        Ok((normalized, full_path))
    }

    /// Set the self-reference after wrapping in Arc. Must be called once after Arc::new.
    pub fn set_self_arc(&self, arc: &Arc<LucidosEngine>) {
        self.self_arc.set(Arc::downgrade(arc)).ok();
    }

    /// Clone the Arc<Self> for spawning background tasks.
    fn clone_arc(&self) -> Arc<LucidosEngine> {
        self.self_arc
            .get()
            .expect("self_arc not initialized")
            .upgrade()
            .expect("engine dropped while in use")
    }

    /// Get the workspace path
    pub fn workspace_path(&self) -> &std::path::Path {
        &self.workspace_path
    }

    /// Get the shared knowhow directory (~/.lucidos/knowhow), if available
    pub fn shared_knowhow_dir(&self) -> Option<PathBuf> {
        self.user_dir.as_ref().map(|ud| ud.join("knowhow"))
    }

    /// Get the engine-shipped system knowhow directory (`<repo_root>/system-knowhow/`).
    pub fn system_knowhow_dir(&self) -> Option<&std::path::Path> {
        self.system_knowhow_dir.as_deref()
    }

    /// Bundle the user-curated knowhow search directories (shared + local).
    /// System knowhow is loaded separately via [`crate::core::SystemKnowhowStore`].
    pub fn knowhow_dirs(&self) -> crate::core::knowhow::KnowhowDirs {
        crate::core::knowhow::KnowhowDirs {
            shared: self.shared_knowhow_dir(),
            local: self.workspace_path.join(crate::core::KNOWHOW_DIR),
        }
    }

    /// Get the user-level Lucidos directory (~/.lucidos), if available
    pub fn user_dir(&self) -> Option<&std::path::Path> {
        self.user_dir.as_deref()
    }

    /// Get a human-readable workspace name (last path component, or full path if root)
    pub fn workspace_name(&self) -> String {
        self.workspace_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.workspace_path.to_string_lossy().to_string())
    }

    /// Get reference to the app manager
    pub fn app_manager(&self) -> &AppManager {
        &self.app_manager
    }

    /// Get the shared database connection pool
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// Record that `(repo_root, branch_name)` has been hardened at `head_sha`.
    /// Called by the `/api/internal/mark-hardened` endpoint that the
    /// `mark-harden.sh` hook hits via `lucidos hardened mark`.
    pub async fn record_hardened(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
        head_sha: &str,
    ) -> Result<(), sqlx::Error> {
        crate::engine::git_ops::record_hardened(&self.pool, repo_root, branch_name, head_sha).await
    }

    pub(crate) async fn harden_marker_state(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> crate::engine::git_ops::HardenMarkerState {
        crate::engine::git_ops::harden_marker_state(&self.pool, repo_root, branch_name).await
    }

    /// Get a clone of the event store for sharing with read-only handlers
    pub fn event_store(&self) -> &EventStore {
        &self.event_store
    }

    /// In-memory event-sourced projection of changes (pending + applied +
    /// discarded + reverted). Backed by the EventBus emit path; rebuilt on
    /// startup from the events table.
    pub fn changes(&self) -> &crate::core::changes_projection::ChangesProjection {
        self.event_bus.changes_projection()
    }

    /// Persist a domain event to the events table and broadcast it on the EventBus.
    /// Used by the LLM `emit_event` tool and the HTTP API for non-transient emits.
    pub async fn emit_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let result = self
            .emit_domain_event_inner(event_type, payload, false)
            .await?;
        Ok(result
            .expect("non-transient DomainEvent always returns EmitResult")
            .event_id)
    }

    /// Broadcast a domain event on SSE without writing it to the events table.
    /// Used for high-churn coordination signals (heartbeats, presenter↔remote
    /// state) where the audit trail isn't valuable.
    pub async fn broadcast_transient_domain_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit_domain_event_inner(event_type, payload, true)
            .await?;
        Ok(())
    }

    async fn emit_domain_event_inner(
        &self,
        event_type: &str,
        payload: serde_json::Value,
        transient: bool,
    ) -> Result<
        Option<crate::engine::event_bus::EmitResult>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let depth = crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH
            .try_with(|d| *d)
            .unwrap_or(0);
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::DomainEvent {
                    event_type: event_type.to_string(),
                    payload,
                    depth,
                    transient,
                },
            ))
            .await
    }

    /// Update a trigger's `last_run` in memory and persist a `TriggerExecuted`
    /// event via EventBus. Used by both cron and event-based trigger execution.
    pub async fn record_trigger_executed(&self, trigger_id: &str) {
        let now = chrono::Utc::now();
        {
            let mut configs = self.trigger_configs.write().unwrap();
            if let Some(c) = configs.get_mut(trigger_id) {
                c.last_run = Some(now);
            }
        }
        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::TriggerExecuted {
                    trigger_id: trigger_id.to_string(),
                    payload: serde_json::json!({
                        "trigger_id": trigger_id,
                        "last_run": now.to_rfc3339(),
                    }),
                },
            ))
            .await
        {
            log!("[Triggers] Failed to persist TriggerExecuted event: {}", e);
        }
    }

    /// Get a reference to the embedder for sharing with read-only handlers
    pub fn embedder(&self) -> &Arc<FastEmbedProvider> {
        &self.embedder
    }

    /// Get a reference to the memory index for sharing with read-only handlers
    pub fn memory_index(&self) -> &Option<PgVectorIndex> {
        &self.memory_index
    }

    pub fn is_rebuilding_memory(&self) -> bool {
        self.rebuilding_memory.load(Ordering::SeqCst)
    }

    pub fn cancel_memory_rebuild(&self) {
        self.cancel_rebuild.store(true, Ordering::SeqCst);
    }

    /// Register a new active thread (sync, no queuing). Used by callers that
    /// know the thread is free (e.g., CC tool-spawned threads with unique IDs).
    pub fn register_thread(
        &self,
        thread_id: Uuid,
    ) -> (
        CancellationToken,
        mpsc::UnboundedReceiver<InjectedPrompt>,
        ThreadGuard,
    ) {
        let token = CancellationToken::new();
        let (injection_tx, injection_rx) = mpsc::unbounded_channel();
        let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
        let mut threads = self.active_threads.lock().unwrap();
        threads.insert(
            thread_id,
            ThreadHandle {
                token: token.clone(),
                injection_tx,
                generation: gen,
            },
        );
        let guard = ThreadGuard {
            active_threads: self.active_threads.clone(),
            thread_id,
            completion_notify: self.thread_completion.clone(),
            generation: gen,
        };
        (token, injection_rx, guard)
    }

    /// Register a thread, waiting for any existing request on the same thread
    /// to finish first. This queues follow-up messages instead of cancelling
    /// in-progress work. The user must explicitly cancel if they want to
    /// interrupt the current request.
    ///
    /// Safety: if the existing thread doesn't finish within 60 seconds, it is
    /// force-cancelled and evicted. This prevents follow-up messages from
    /// hanging forever if a CC task gets stuck (e.g., process crash without
    /// proper guard cleanup).
    pub async fn register_thread_queued(
        &self,
        thread_id: Uuid,
    ) -> (
        CancellationToken,
        mpsc::UnboundedReceiver<InjectedPrompt>,
        ThreadGuard,
    ) {
        let wait_result = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let n = {
                    let threads = self.active_threads.lock().unwrap();
                    if threads.contains_key(&thread_id) {
                        let mut completions = self.thread_completion.lock().unwrap();
                        completions
                            .entry(thread_id)
                            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
                            .clone()
                    } else {
                        return;
                    }
                };
                log!(
                    "[Chat] Thread {} is busy, queuing follow-up request",
                    thread_id
                );
                // 100ms fallback guards against missed notify_waiters() — if the
                // notification fired between contains_key and .await, we
                // retry after 100ms and re-check the map.
                tokio::select! {
                    _ = n.notified() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
        })
        .await;

        if wait_result.is_err() {
            log!(
                "[Chat] Thread {} stuck for 60s — force-cancelling and evicting",
                thread_id
            );
            // Engine-initiated abort — emit ResponseAborted with actor=System
            // BEFORE cancelling the token. Without this, the downstream cancel
            // arms default to ResponseCanceled (because they read
            // is_shutdown=false), which the frontend renders as user-initiated
            // "Canceled" — misleading users into thinking they pressed Stop.
            emit_stuck_thread_eviction_abort(
                &self.event_bus,
                &self.pool,
                &self.agent_sessions,
                thread_id,
            )
            .await;
            if let Some(handle) = self.active_threads.lock().unwrap().remove(&thread_id) {
                handle.token.cancel();
            }
            if let Some(n) = self.thread_completion.lock().unwrap().remove(&thread_id) {
                n.notify_waiters();
            }
        }

        self.register_thread(thread_id)
    }

    /// Cancel a specific thread. Returns `true` if the thread had an active
    /// `cancel_token` registered (the cancel landed and the per-thread loop
    /// will observe it). Returns `false` when there is no active entry — the
    /// caller can then fall back to settling the projection.
    pub fn cancel_thread(&self, thread_id: Uuid) -> bool {
        if let Some(handle) = self.active_threads.lock().unwrap().get(&thread_id) {
            handle.token.cancel();
            true
        } else {
            false
        }
    }

    /// Inject a prompt into an active thread's agentic loop.
    /// Returns true if the thread was active and the message was queued.
    pub fn inject_prompt(
        &self,
        thread_id: Uuid,
        message: String,
        event_id: Option<Uuid>,
        mode: thread_events::ActorMode,
        spawning_event_id: Option<Uuid>,
        origin: Option<thread_events::MessageOrigin>,
    ) -> bool {
        if let Some(handle) = self.active_threads.lock().unwrap().get(&thread_id) {
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: message,
                    event_id,
                    mode,
                    spawning_event_id,
                    images: None,
                    origin,
                })
                .is_ok()
        } else {
            false
        }
    }

    /// Get a reference to the memory extractor (for Flash title generation, etc.)
    pub fn extractor(&self) -> Option<&crate::memory::MemoryExtractor> {
        self.extractor.as_ref()
    }

    /// Get list of thread IDs with a live processing task (chat loop running).
    /// Does NOT include idle CC sessions — those are tracked via thread_summaries.status.
    pub fn processing_thread_ids(&self) -> Vec<Uuid> {
        self.active_threads
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect()
    }

    /// Drain any messages from an injection channel that were not consumed
    /// by the agentic loop. These are follow-up messages that arrived via
    /// inject_prompt() after the loop's last try_recv() but before the
    /// ThreadGuard dropped — a race window where the thread appears active
    /// but nobody is reading the injection channel.
    pub(crate) fn drain_orphaned_injections(
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        let mut orphans = Vec::new();
        while let Ok(prompt) = injection_rx.try_recv() {
            orphans.push(prompt);
        }
        orphans
    }

    /// Spawn background title generation for a thread (used when pinning).
    /// Looks up the first message of the thread and generates a title via Flash.
    pub async fn spawn_title_generation(&self, thread_id: &str) {
        let tid_uuid = match uuid::Uuid::parse_str(thread_id) {
            Ok(u) => u,
            Err(_) => return,
        };
        if let Some(ref extractor) = self.extractor {
            let title_model = PreferenceStore::get(&self.pool, PREF_MODEL_TITLE)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let provider = extractor.provider_for_model(&title_model);
            let event_store = self.event_store.clone();
            let bus = self.event_bus.clone();
            let tid = thread_id.to_string();
            tokio::spawn(async move {
                let (first_msg, image_desc) = match event_store.get_thread_first_message(&tid).await
                {
                    Ok(Some((msg, desc))) if !msg.is_empty() => (msg, desc),
                    Ok(_) => return,
                    Err(e) => {
                        log!("[Thread] Failed to get first message for title: {}", e);
                        return;
                    }
                };

                chat::emit_generated_title(
                    &bus,
                    &provider,
                    tid_uuid,
                    &first_msg,
                    image_desc.as_deref(),
                    None,
                )
                .await;
            });
        }
    }

    /// Cancel all active threads.
    pub fn cancel_all_threads(&self) {
        for handle in self.active_threads.lock().unwrap().values() {
            handle.token.cancel();
        }
    }

    /// Pre-shutdown abort emission for `/api/restart`. Walks the in-flight
    /// chat AND CC threads and emits the boundary events with a user-attributed
    /// `actor` so the post-restart timeline reads "You restarted" instead of
    /// "⚙ System restarted".
    ///
    /// For chat threads: emits `ResponseAborted { actor: <actor> }` with
    /// `request_event_id` pointing to the originating MessageReceived/
    /// TriggerStarted.
    ///
    /// For CC threads: emits both `ResponseAborted` AND the synthetic
    /// `CodingAgentIdled { reason: engine_restart_interrupt }` so the spawn
    /// dispatcher's classifier (which runs after restart on recovery) sees a
    /// thread that's already terminated and skips re-emitting the same pair.
    /// `actor` flows onto both events.
    ///
    /// Idempotent — reading the per-event guard inside the projection ensures
    /// a duplicate restart click does not double-emit.
    pub async fn abort_in_flight_for_restart(
        self: &std::sync::Arc<Self>,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};

        // `all_cc_thread_ids` covers idle CC sessions too — their run loop
        // stays registered in `active_threads` between turns, so they show up
        // in `processing_thread_ids()` and must be excluded from the chat
        // bucket. `cc_thread_ids` (in-flight only) and `external_emitted_flags`
        // drive the pre-emit; the flag tells `run_session`'s classify/safety
        // paths to skip a duplicate emit (see `external_terminal_emitted`).
        let (all_cc_thread_ids, cc_thread_ids, external_emitted_flags): (
            std::collections::HashSet<uuid::Uuid>,
            Vec<uuid::Uuid>,
            Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        ) = {
            let guard = self.agent_sessions.lock().await;
            let all = guard.keys().copied().collect();
            let (ids, flags) = guard
                .iter()
                .filter(|(_, s)| s.is_in_flight())
                .map(|(tid, s)| (*tid, s.external_terminal_emitted.clone()))
                .unzip();
            (all, ids, flags)
        };

        let chat_thread_ids =
            partition_chat_thread_ids(&self.processing_thread_ids(), &all_cc_thread_ids);

        // ---- Chat threads ---------------------------------------------------
        // Look up originating event ids in parallel — sequential awaits would
        // serialize N round-trips on a busy restart.
        let chat_originating_ids: Vec<Option<uuid::Uuid>> = futures::future::join_all(
            chat_thread_ids.iter().map(|tid| {
                crate::engine::agent_session::latest_originating_event_id(
                    &self.pool,
                    *tid,
                    &["MessageReceived", "TriggerStarted"],
                )
            }),
        )
        .await;
        for (thread_id, originating_event_id) in chat_thread_ids.iter().zip(chat_originating_ids) {
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id: *thread_id,
                        event: ThreadEvent::ResponseAborted {
                            text: "This response was interrupted by an engine restart.".to_string(),
                            images: vec![],
                            model: None,
                            reasoning_effort: None,
                        },
                        meta: EventMeta {
                            request_event_id: originating_event_id,
                            actor: actor.clone(),
                            ..EventMeta::NONE
                        },
                    },
                    "[Restart] ResponseAborted (chat)",
                )
                .await;
        }

        // ---- CC threads -----------------------------------------------------
        // Pre-emit ONLY the boundary `ResponseAborted{actor: device}` so the
        // post-restart timeline reads "You restarted" on the AbortPanel.
        // The synthetic `CodingAgentIdled{engine_restart_interrupt}` that
        // drives the spawn-dispatcher classifier is left to the post-restart
        // recovery sweep — it owns the decision of whether to preserve the
        // worktree (the worktree is `--resume`'d by the user's Continue click)
        // or clean it up. Pre-emitting that idle event from here would push
        // the branch into `idle_branches` on restart and trigger a worktree
        // cleanup, breaking the Continue flow.
        let cc_originating_ids: Vec<Option<uuid::Uuid>> = futures::future::join_all(
            cc_thread_ids.iter().map(|tid| {
                crate::engine::agent_session::latest_originating_event_id(
                    &self.pool,
                    *tid,
                    &["MessageReceived", "CodingAgentUserMessageSent", "TriggerStarted"],
                )
            }),
        )
        .await;
        for ((thread_id, originating_event_id), flag) in cc_thread_ids
            .iter()
            .zip(cc_originating_ids)
            .zip(external_emitted_flags)
        {
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id: *thread_id,
                        event: ThreadEvent::ResponseAborted {
                            text: String::new(),
                            images: vec![],
                            model: None,
                            reasoning_effort: None,
                        },
                        meta: EventMeta {
                            channel: Some(EventChannel::CodingAgent),
                            request_event_id: originating_event_id,
                            actor: actor.clone(),
                            ..EventMeta::NONE
                        },
                    },
                    "[Restart] ResponseAborted (cc)",
                )
                .await;
            // Set AFTER the emit lands so any Result arriving from this point
            // on observes the flag and skips its own duplicate emit.
            flag.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Emit ResponseAborted for all active non-CC threads during engine shutdown.
    /// CC threads are handled separately by `shutdown_agent_sessions`.
    ///
    /// After emitting, cancels all threads so their tasks can clean up. The
    /// agentic loop may also emit ResponseCanceled on cancellation — having both
    /// is harmless (ResponseAborted takes precedence in status derivation).
    ///
    /// Stamps `actor: System` so the AbortPanel renders ⚙ System — the host
    /// system killed these in-flight responses (engine shutdown). The
    /// user-driven `/api/restart` path pre-emits with `actor: Device {..}`
    /// BEFORE shutdown for in-flight threads it knows about; this fallback
    /// covers anything that started after that pre-emit.
    pub async fn shutdown_active_threads(&self) {
        let active_ids = self.processing_thread_ids();
        if active_ids.is_empty() {
            return;
        }
        // CC threads (in-flight or idle) are handled by shutdown_agent_sessions.
        let all_cc_thread_ids: std::collections::HashSet<uuid::Uuid> =
            self.agent_sessions.lock().await.keys().copied().collect();
        for thread_id in partition_chat_thread_ids(&active_ids, &all_cc_thread_ids) {
            log!(
                "[Shutdown] Emitting ResponseAborted for active thread {}",
                thread_id
            );
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
                        text: "This response was interrupted by an engine shutdown.".to_string(),
                        images: vec![],
                        model: None,
                        reasoning_effort: None,
                    },
                    meta: crate::engine::thread_events::EventMeta::with_actor(Some(
                        crate::engine::thread_events::MessageOrigin::system(),
                    )),
                })
                .await
            {
                log!(
                    "[Shutdown] Failed to emit ResponseAborted for thread {}: {}",
                    thread_id,
                    e
                );
            }
        }
        self.cancel_all_threads();
    }

    pub async fn shutdown_browser(&self) {
        if let Err(e) = self.browser_runtime.close_all().await {
            log!("[Engine] Error closing browsers on shutdown: {}", e);
        }
    }

    /// Gracefully stop all running Claude Code sessions.
    /// Sends interrupt to active sessions, waits for them to produce
    /// a Result event and go idle (persisting cc_session_id in CodingAgentIdled),
    /// then cancels remaining sessions.
    pub async fn shutdown_agent_sessions(&self) {
        // Mark all sessions as shutting down and collect their interrupt/cancel handles.
        // CC cleanup reads `shutting_down` to emit SessionEnded { reason: "shutdown" }
        // instead of "completed" — the frontend uses this to show "Aborted".
        let sessions: Vec<(
            uuid::Uuid,
            std::sync::Arc<tokio::sync::Notify>,
            std::sync::Arc<tokio::sync::Notify>,
        )> = {
            let guard = self.agent_sessions.lock().await;
            for s in guard.values() {
                s.shutting_down
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            guard
                .iter()
                .map(|(tid, s)| (*tid, s.interrupt.clone(), s.cancel.clone()))
                .collect()
        };

        if sessions.is_empty() {
            return;
        }

        log!(
            "[Shutdown] Gracefully stopping {} Claude Code session(s)...",
            sessions.len()
        );

        // Phase 1: Interrupt all active sessions (like pressing Esc).
        // This makes CC stop current work and emit a Result event, which triggers
        // CodingAgentIdled (with cc_session_id) to be persisted to DB.
        for (tid, interrupt, _) in &sessions {
            log!("[Shutdown] Interrupting CC session {}", tid);
            interrupt.notify_one();
        }

        // Phase 2: Poll until all sessions are gone or 10 seconds elapse.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let remaining = self.agent_sessions.lock().await.len();
            if remaining == 0 {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                log!(
                    "[Shutdown] {} session(s) still active after timeout — force-cancelling",
                    remaining
                );
                for (tid, _, cancel) in &sessions {
                    if self.agent_sessions.lock().await.contains_key(tid) {
                        cancel.notify_one();
                    }
                }
                // Brief wait for cancel cleanup
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        log!("[Shutdown] Claude Code sessions stopped.");
    }

    /// Check if user profile exists
    pub async fn has_user_profile(&self) -> bool {
        !self.user_profile.read().await.is_empty()
    }

    /// Record a trigger completion.
    /// LLM triggers have a real thread_id and go through EventBus as a thread event.
    /// Script triggers have no thread — they use a system event.
    pub async fn record_trigger_completed(
        &self,
        trigger_id: &str,
        trigger_name: &str,
        result_summary: &str,
        thread_id: Option<Uuid>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(tid) = thread_id {
            self.event_bus
                .emit(event_bus::BusEvent::Thread {
                    thread_id: tid,
                    event: thread_events::ThreadEvent::TriggerCompleted {
                        trigger_id: trigger_id.to_string(),
                        trigger_name: Some(trigger_name.to_string()),
                        result_summary: Some(result_summary.to_string()),
                    },
                    meta: thread_events::EventMeta {
                        channel: Some(crate::engine::thread_events::EventChannel::Trigger),
                        ..thread_events::EventMeta::NONE
                    },
                })
                .await?;
        } else {
            self.event_bus
                .emit(crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::TriggerCompleted {
                        trigger_id: trigger_id.to_string(),
                        trigger_name: trigger_name.to_string(),
                        result_summary: result_summary.to_string(),
                    },
                ))
                .await?;
        }
        Ok(())
    }

    /// Get a snapshot of the conversation at a specific event (thin wrapper)
    pub async fn get_conversation_at_event(
        &self,
        event_id: Uuid,
    ) -> Result<ConversationSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        self.event_store
            .get_conversation_at_event(event_id, &self.workspace_path)
            .await
    }

    /// Build CRED_*, OAUTH_*, LUCIDOS_WORKSPACE, and PATH environment variables
    /// for script execution. Used by `execute_python_tool` (LLM),
    /// `execute_bash_tool`, and `execute_script` (scheduled tasks).
    ///
    /// `LUCIDOS_WORKSPACE` + the `.lucidos/bin` symlink let scripts call
    /// `lucidos data write` / `lucidos events emit` / `lucidos events query`
    /// instead of hand-rolling HTTP requests back to the engine.
    pub(crate) async fn build_script_env_vars(&self) -> Vec<(String, String)> {
        use crate::core::oauth;
        use crate::core::{CredentialStore, OAuthStore};
        use crate::runtime::lucidos_cli::{lucidos_cli_dir, workspace_script_env_vars};

        let mut env_vars = workspace_script_env_vars(self.workspace_path(), lucidos_cli_dir());

        // Credentials → CRED_* vars
        match CredentialStore::list_all_with_secrets(&self.pool).await {
            Ok(creds) => env_vars.extend(Self::credential_env_vars(creds)),
            Err(e) => log!(
                "[Python] Failed to load credentials for env injection: {}",
                e
            ),
        }

        // OAuth accounts → OAUTH_* vars (auto-refreshed)
        match OAuthStore::list_all_with_tokens(&self.pool).await {
            Ok(mut accounts) => {
                for account in &mut accounts {
                    if let Err(e) = oauth::refresh_oauth_if_needed(&self.pool, account).await {
                        log!(
                            "[Python] OAuth refresh failed for {}: {}",
                            account.provider,
                            e
                        );
                    }
                }
                env_vars.extend(Self::oauth_env_vars(accounts));
            }
            Err(e) => log!(
                "[Python] Failed to load OAuth accounts for env injection: {}",
                e
            ),
        }

        env_vars
    }

    /// Execute a script file by workspace-relative path. Used by scheduled script tasks.
    ///
    /// Runtime is determined by file extension:
    /// - `.py` → Python (sandboxed venv)
    /// - `.sh` → Bash (`/bin/sh`)
    pub async fn execute_script(
        &self,
        script_path: &str,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if script_path.contains("..")
            || script_path.starts_with('/')
            || script_path.starts_with('\\')
        {
            return Err("Invalid script path: must be relative, no '..'".into());
        }

        let full_path = self.workspace_path.join(script_path);
        if !full_path.exists() {
            return Err(format!("Script not found: {}", script_path).into());
        }

        let extension = std::path::Path::new(script_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // Script args + shared credentials/OAuth env vars (injected into ALL runtimes)
        let mut env_vars = vec![
            ("LUCIDOS_SCRIPT_PATH".to_string(), script_path.to_string()),
            (
                "LUCIDOS_ARGS".to_string(),
                serde_json::to_string(args).unwrap_or_default(),
            ),
        ];
        for (i, arg) in args.iter().enumerate() {
            env_vars.push((format!("LUCIDOS_ARG_{}", i), arg.clone()));
        }
        env_vars.extend(self.build_script_env_vars().await);
        env_vars.extend(extra_env.iter().cloned());

        let output = match extension {
            "py" => {
                let code = std::fs::read_to_string(&full_path)
                    .map_err(|e| format!("Failed to read script {}: {}", script_path, e))?;
                self.python_runtime
                    .execute_with_env(&code, env_vars)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?
            }
            "sh" => self.execute_shell_script(&full_path, env_vars).await?,
            _ => {
                let msg = match crate::triggers::validate_script_extension(script_path) {
                    Err(e) => e,
                    Ok(()) => format!("No runtime configured for '.{}' scripts", extension),
                };
                return Err(msg.into());
            }
        };

        // Auto-commit any files the script touched under artifacts/
        self.commit_dirty_logged("Script task output", script_path)
            .await;

        Ok(output)
    }

    async fn execute_shell_script(
        &self,
        script_path: &std::path::Path,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::sanitize_for_jsonb;

        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg(script_path)
            .current_dir(self.workspace_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, value) in &env_vars {
            cmd.env(key, value);
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn shell script: {}", e))?;

        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(300),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("Error executing shell script: {}", e).into()),
            Err(_) => return Err("Shell script timed out after 300s".into()),
        };

        let stdout = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stdout));
        let stderr = sanitize_for_jsonb(&String::from_utf8_lossy(&output.stderr));

        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!(
                "Shell script error (exit {}):\n{}",
                output.status.code().unwrap_or(-1),
                stderr
            )
            .into())
        }
    }

    /// Commit all dirty data/ files with a 30s timeout, logging success/failure.
    /// Shared by all code paths that may produce dirty files (scripts, Claude Code, run_python).
    pub(crate) async fn commit_dirty_logged(&self, message: &str, context: &str) {
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.artifact_manager.commit_all_dirty(message),
        )
        .await
        {
            Ok(Ok(Some(commit))) => {
                log!(
                    "[Engine] Auto-committed dirty data files after {} ({})",
                    context,
                    &commit[..commit.floor_char_boundary(7)]
                );
            }
            Ok(Err(e)) => {
                log!("[Engine] Failed to commit dirty data files after {}: {}", context, e);
            }
            Err(_) => {
                log!("[Engine] commit_all_dirty timed out (30s) after {}", context);
            }
            Ok(Ok(None)) => {}
        }
    }
}

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
/// this pre-emit, the cancel arms would default to `ResponseCanceled`
/// (`is_shutdown=false`) and the user would see a misleading "Canceled".
/// CC sessions also get `external_terminal_emitted` set so the run-loop
/// arm skips its duplicate emit; chat threads' `agentic_loop` may still
/// emit a duplicate `ResponseCanceled`, which the frontend deflates
/// because `Aborted` is checked before `Canceled` in `exchangeStatus`.
pub(crate) async fn emit_stuck_thread_eviction_abort(
    bus: &event_bus::EventBus,
    pool: &sqlx::PgPool,
    agent_sessions: &tokio::sync::Mutex<HashMap<Uuid, types::AgentSession>>,
    thread_id: Uuid,
) {
    use thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};

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

    bus.emit_or_log(
        event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: String::new(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta {
                channel,
                request_event_id,
                actor: Some(MessageOrigin::system()),
                ..EventMeta::NONE
            },
        },
        "[Engine] ResponseAborted (stuck-thread eviction)",
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    /// Create a standalone active_threads map for testing thread registration.
    fn make_threads() -> Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>> {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// Create a standalone completion notifiers map for testing.
    fn make_completions() -> Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>> {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// Replicate register_thread logic for standalone testing (sync, no queuing).
    fn register(
        threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
        thread_id: Uuid,
    ) -> (
        CancellationToken,
        mpsc::UnboundedReceiver<InjectedPrompt>,
        ThreadGuard,
    ) {
        let token = CancellationToken::new();
        let (injection_tx, injection_rx) = mpsc::unbounded_channel();
        let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
        let mut guard_map = threads.lock().unwrap();
        guard_map.insert(
            thread_id,
            ThreadHandle {
                token: token.clone(),
                injection_tx,
                generation: gen,
            },
        );
        let guard = ThreadGuard {
            active_threads: threads.clone(),
            thread_id,
            completion_notify: Arc::new(std::sync::Mutex::new(HashMap::new())),
            generation: gen,
        };
        (token, injection_rx, guard)
    }

    /// Replicate register_thread_queued logic: waits for existing thread to
    /// finish before registering. Force-evicts after `timeout`.
    async fn register_queued_with_timeout(
        threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
        completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
        thread_id: Uuid,
        timeout: std::time::Duration,
    ) -> (
        CancellationToken,
        mpsc::UnboundedReceiver<InjectedPrompt>,
        ThreadGuard,
    ) {
        let wait_result = tokio::time::timeout(timeout, async {
            loop {
                let n = {
                    let guard_map = threads.lock().unwrap();
                    if guard_map.contains_key(&thread_id) {
                        let mut comps = completions.lock().unwrap();
                        comps
                            .entry(thread_id)
                            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
                            .clone()
                    } else {
                        return;
                    }
                };
                tokio::select! {
                    _ = n.notified() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
                }
            }
        })
        .await;

        if wait_result.is_err() {
            if let Some(handle) = threads.lock().unwrap().remove(&thread_id) {
                handle.token.cancel();
            }
            if let Some(n) = completions.lock().unwrap().remove(&thread_id) {
                n.notify_waiters();
            }
        }

        let token = CancellationToken::new();
        let (injection_tx, injection_rx) = mpsc::unbounded_channel();
        let gen = THREAD_GENERATION.fetch_add(1, Ordering::Relaxed);
        let mut guard_map = threads.lock().unwrap();
        guard_map.insert(
            thread_id,
            ThreadHandle {
                token: token.clone(),
                injection_tx,
                generation: gen,
            },
        );
        let guard = ThreadGuard {
            active_threads: threads.clone(),
            thread_id,
            completion_notify: completions.clone(),
            generation: gen,
        };
        (token, injection_rx, guard)
    }

    /// Convenience wrapper with the default 60s timeout.
    async fn register_queued(
        threads: &Arc<std::sync::Mutex<HashMap<Uuid, ThreadHandle>>>,
        completions: &Arc<std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Notify>>>>,
        thread_id: Uuid,
    ) -> (
        CancellationToken,
        mpsc::UnboundedReceiver<InjectedPrompt>,
        ThreadGuard,
    ) {
        register_queued_with_timeout(
            threads,
            completions,
            thread_id,
            std::time::Duration::from_secs(60),
        )
        .await
    }

    #[test]
    fn register_thread_creates_fresh_token() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (token, _injection_rx, _guard) = register(&threads, tid);
        assert!(!token.is_cancelled());
        assert!(threads.lock().unwrap().contains_key(&tid));
    }

    #[test]
    fn guard_drop_removes_from_map() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (token, _injection_rx, guard) = register(&threads, tid);
        assert!(threads.lock().unwrap().contains_key(&tid));
        drop(guard);
        assert!(!threads.lock().unwrap().contains_key(&tid));
        // Token is NOT cancelled by guard drop — only removed from map
        assert!(!token.is_cancelled());
    }

    #[test]
    fn guard_drop_does_not_cancel_token() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (token, _injection_rx, guard) = register(&threads, tid);
        drop(guard);
        // Critical: dropping guard must NOT cancel the token
        // This ensures CC sessions aren't killed when guards drop
        assert!(!token.is_cancelled(), "guard drop must not cancel token");
    }

    #[test]
    fn reregister_same_id_replaces_token_without_cancel() {
        // register_thread (sync) replaces the token in the map but does NOT
        // cancel the old one. Cancellation is only done by explicit cancel_thread.
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (old_token, _old_rx, _old_guard) = register(&threads, tid);
        let (new_token, _new_rx, _new_guard) = register(&threads, tid);
        assert!(
            !old_token.is_cancelled(),
            "old token must NOT be cancelled on re-register"
        );
        assert!(!new_token.is_cancelled(), "new token must not be cancelled");
    }

    #[test]
    fn different_ids_dont_interfere() {
        let threads = make_threads();
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();
        let (token_a, _rx_a, _guard_a) = register(&threads, tid_a);
        let (token_b, _rx_b, _guard_b) = register(&threads, tid_b);
        assert!(!token_a.is_cancelled());
        assert!(!token_b.is_cancelled());
        // Dropping one doesn't cancel the other
        drop(_guard_a);
        assert!(!token_b.is_cancelled());
    }

    #[test]
    fn cancel_thread_cancels_correct_token() {
        let threads = make_threads();
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();
        let (token_a, _rx_a, _guard_a) = register(&threads, tid_a);
        let (token_b, _rx_b, _guard_b) = register(&threads, tid_b);

        // Cancel only thread A
        if let Some(handle) = threads.lock().unwrap().get(&tid_a) {
            handle.token.cancel();
        }
        assert!(token_a.is_cancelled());
        assert!(!token_b.is_cancelled(), "cancelling A must not cancel B");
    }

    #[test]
    fn cancel_all_threads_cancels_all() {
        let threads = make_threads();
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();
        let (token_a, _rx_a, _guard_a) = register(&threads, tid_a);
        let (token_b, _rx_b, _guard_b) = register(&threads, tid_b);

        for handle in threads.lock().unwrap().values() {
            handle.token.cancel();
        }
        assert!(token_a.is_cancelled());
        assert!(token_b.is_cancelled());
    }

    #[test]
    fn cc_spawn_scenario_original_guard_drop_does_not_cancel_cc() {
        // Simulates: original chat registers thread_id=A, spawns CC with thread_id=B.
        // When original chat completes, guard_A drops. CC's token_B must NOT be cancelled.
        let threads = make_threads();
        let original_tid = Uuid::new_v4();
        let cc_tid = Uuid::new_v4();

        let (_token_orig, _rx_orig, guard_orig) = register(&threads, original_tid);
        let (token_cc, _rx_cc, _guard_cc) = register(&threads, cc_tid);

        // Original chat completes, drops its guard
        drop(guard_orig);

        assert!(
            !token_cc.is_cancelled(),
            "CC token must survive original thread guard drop"
        );
    }

    #[tokio::test]
    async fn idle_wait_exits_on_cancel_notify() {
        let threads = make_threads();
        let cc_tid = Uuid::new_v4();
        let (token, _injection_rx, _guard) = register(&threads, cc_tid);
        let cancel = Arc::new(tokio::sync::Notify::new());

        let token_clone = token.clone();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_clone.notified() => {
                        return "cancel_notified";
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                }
                if token_clone.is_cancelled() {
                    return "token_cancelled";
                }
            }
        });

        // Notify cancel after 50ms
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.notify_one();

        let result = tokio::time::timeout(std::time::Duration::from_millis(200), handle)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, "cancel_notified");
    }

    #[tokio::test]
    async fn register_thread_queues_instead_of_cancelling() {
        // When a second request arrives for the same thread while the first is
        // still running, register_thread_queued must WAIT for the first to finish
        // instead of cancelling it.
        let threads = make_threads();
        let completions = make_completions();
        let tid = Uuid::new_v4();

        // First request registers and starts "processing"
        let (token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;
        assert!(!token1.is_cancelled());

        // Second request arrives — spawned so it can await
        let threads_c = threads.clone();
        let completions_c = completions.clone();
        let second =
            tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid).await });

        // Give the second request time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // First request's token must NOT be cancelled while it's still processing
        assert!(
            !token1.is_cancelled(),
            "first token must not be cancelled by queued request"
        );

        // Second request should still be waiting
        assert!(
            !second.is_finished(),
            "second request must be waiting, not done"
        );

        // First request finishes — drop guard (triggers notify)
        drop(guard1);

        // Second request should now proceed
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
        assert!(
            result.is_ok(),
            "second request must unblock after first completes"
        );
        let (token2, _rx2, _guard2) = result.unwrap().unwrap();
        assert!(!token2.is_cancelled(), "second token must be fresh");
        assert!(
            !token1.is_cancelled(),
            "first token completed naturally, never cancelled"
        );
    }

    #[tokio::test]
    async fn explicit_cancel_unblocks_queued_request() {
        // When the first request is explicitly cancelled (via cancel_thread),
        // the queued second request should proceed after the guard drops.
        let threads = make_threads();
        let completions = make_completions();
        let tid = Uuid::new_v4();

        let (token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;

        let threads_c = threads.clone();
        let completions_c = completions.clone();
        let second =
            tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid).await });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!second.is_finished(), "second must be waiting");

        // Explicitly cancel the first request's token
        token1.cancel();
        // Guard drop triggers the notify — simulates the cancelled task exiting
        drop(guard1);

        let result = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
        assert!(
            result.is_ok(),
            "second request must unblock after cancel + guard drop"
        );
        let (token2, _rx2, _guard2) = result.unwrap().unwrap();
        assert!(
            !token2.is_cancelled(),
            "second token must be fresh after cancel"
        );
    }

    #[tokio::test]
    async fn multiple_queued_requests_process_in_order() {
        // Three requests for the same thread — they should process sequentially.
        let threads = make_threads();
        let completions = make_completions();
        let tid = Uuid::new_v4();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // First request
        let (_token1, _rx1, guard1) = register_queued(&threads, &completions, tid).await;
        order.lock().unwrap().push(1);

        // Second request (queued)
        let threads_c = threads.clone();
        let completions_c = completions.clone();
        let order_c = order.clone();
        let second = tokio::spawn(async move {
            let (t, _rx, g) = register_queued(&threads_c, &completions_c, tid).await;
            order_c.lock().unwrap().push(2);
            (t, g)
        });

        // Third request (queued behind second)
        let threads_c2 = threads.clone();
        let completions_c2 = completions.clone();
        let order_c2 = order.clone();
        let third = tokio::spawn(async move {
            let (t, _rx, g) = register_queued(&threads_c2, &completions_c2, tid).await;
            order_c2.lock().unwrap().push(3);
            (t, g)
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!second.is_finished(), "second must be waiting");
        assert!(!third.is_finished(), "third must be waiting");

        // First finishes
        drop(guard1);
        let result2 = tokio::time::timeout(std::time::Duration::from_millis(200), second).await;
        assert!(result2.is_ok(), "second must unblock");
        let (_token2, guard2) = result2.unwrap().unwrap();

        // Third should still be waiting (second is now active)
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!third.is_finished(), "third must wait for second");

        // Second finishes
        drop(guard2);
        let result3 = tokio::time::timeout(std::time::Duration::from_millis(200), third).await;
        assert!(result3.is_ok(), "third must unblock");

        // Verify order
        assert_eq!(
            *order.lock().unwrap(),
            vec![1, 2, 3],
            "requests must process in order"
        );
    }

    #[tokio::test]
    async fn queued_request_for_different_thread_proceeds_immediately() {
        // Requests for different thread IDs should not block each other.
        let threads = make_threads();
        let completions = make_completions();
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();

        let (_token_a, _rx_a, _guard_a) = register_queued(&threads, &completions, tid_a).await;

        // Request for thread B should proceed immediately (not blocked by A)
        let threads_c = threads.clone();
        let completions_c = completions.clone();
        let other =
            tokio::spawn(async move { register_queued(&threads_c, &completions_c, tid_b).await });

        let result = tokio::time::timeout(std::time::Duration::from_millis(100), other).await;
        assert!(result.is_ok(), "different thread ID must not be blocked");
    }

    #[tokio::test]
    async fn stuck_thread_force_evicted_after_timeout() {
        // When a thread is stuck in active_threads (e.g., CC process that
        // never completes), register_thread_queued must not hang forever.
        // After the timeout, it should force-cancel and evict the stuck thread.
        let threads = make_threads();
        let completions = make_completions();
        let tid = Uuid::new_v4();

        // Register a "stuck" thread that never drops its guard
        let (stuck_token, _rx, _stuck_guard) = register(&threads, tid);
        assert!(!stuck_token.is_cancelled());

        // A follow-up request with a short timeout (200ms for test speed)
        let threads_c = threads.clone();
        let completions_c = completions.clone();
        let second = tokio::spawn(async move {
            register_queued_with_timeout(
                &threads_c,
                &completions_c,
                tid,
                std::time::Duration::from_millis(200),
            )
            .await
        });

        // The follow-up must complete within a reasonable time (timeout + margin)
        let result = tokio::time::timeout(std::time::Duration::from_millis(500), second).await;
        assert!(
            result.is_ok(),
            "follow-up must not hang forever — timeout should evict stuck thread"
        );
        let (token2, _rx2, _guard2) = result.unwrap().unwrap();
        assert!(!token2.is_cancelled(), "new token must be fresh");
        // The stuck thread's token must have been cancelled
        assert!(
            stuck_token.is_cancelled(),
            "stuck thread token must be force-cancelled"
        );
    }

    #[tokio::test]
    async fn old_guard_drop_does_not_remove_new_registration() {
        // After force-eviction, the old ThreadGuard still exists. When it
        // drops, it must NOT remove the new registration (different generation).
        let threads = make_threads();
        let tid = Uuid::new_v4();

        // Register old thread
        let (_old_token, _old_rx, old_guard) = register(&threads, tid);
        assert!(threads.lock().unwrap().contains_key(&tid));

        // Force-evict: remove old handle, register new one (simulates timeout path)
        threads.lock().unwrap().remove(&tid);
        let (_new_token, _new_rx, _new_guard) = register(&threads, tid);
        assert!(threads.lock().unwrap().contains_key(&tid));

        // Drop the old guard — must NOT remove the new registration
        drop(old_guard);
        assert!(
            threads.lock().unwrap().contains_key(&tid),
            "old guard drop must not remove new registration (different generation)"
        );
    }

    #[tokio::test]
    async fn idle_wait_exits_on_token_cancel() {
        let threads = make_threads();
        let cc_tid = Uuid::new_v4();
        let (token, _injection_rx, _guard) = register(&threads, cc_tid);
        let cancel = Arc::new(tokio::sync::Notify::new());

        let token_clone = token.clone();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_clone.notified() => {
                        return "cancel_notified";
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                }
                if token_clone.is_cancelled() {
                    return "token_cancelled";
                }
            }
        });

        // Cancel the token after 50ms (simulates cancel_thread or cancel_all_threads)
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        token.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_millis(200), handle)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, "token_cancelled");
    }

    // --- Injection channel tests ---

    #[test]
    fn inject_prompt_delivers_to_active_thread() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, mut rx, _guard) = register(&threads, tid);

        // Inject a message
        let injected = {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "fix the bug".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .is_ok()
        };
        assert!(injected, "inject_prompt must succeed for active thread");

        // Receiver should have the message
        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.text, "fix the bug");
    }

    #[test]
    fn inject_prompt_fails_for_unknown_thread() {
        let threads = make_threads();
        let tid = Uuid::new_v4();

        // No thread registered — inject should fail
        let result = {
            let map = threads.lock().unwrap();
            map.get(&tid).map(|h| {
                h.injection_tx
                    .send(InjectedPrompt {
                        text: "msg".into(),
                        event_id: None,
                        mode: thread_events::ActorMode::Human,
                        spawning_event_id: None,
                        images: None,
                        origin: None,
                    })
                    .is_ok()
            })
        };
        assert!(result.is_none(), "inject must fail for unknown thread");
    }

    #[test]
    fn inject_prompt_fails_after_guard_drop() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, _rx, guard) = register(&threads, tid);

        // Drop the guard — thread is deregistered
        drop(guard);

        let result = {
            let map = threads.lock().unwrap();
            map.get(&tid).map(|h| {
                h.injection_tx
                    .send(InjectedPrompt {
                        text: "msg".into(),
                        event_id: None,
                        mode: thread_events::ActorMode::Human,
                        spawning_event_id: None,
                        images: None,
                        origin: None,
                    })
                    .is_ok()
            })
        };
        assert!(
            result.is_none(),
            "inject must fail after thread deregistered"
        );
    }

    #[test]
    fn inject_multiple_prompts_drains_in_order() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, mut rx, _guard) = register(&threads, tid);

        // Send multiple injections
        {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "first".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "second".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "third".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
        }

        // Drain all — should come in order
        let mut texts = Vec::new();
        while let Ok(prompt) = rx.try_recv() {
            texts.push(prompt.text);
        }
        assert_eq!(texts, vec!["first", "second", "third"]);
    }

    #[test]
    fn inject_prompt_does_not_cancel_thread() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (token, _rx, _guard) = register(&threads, tid);

        {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "correction".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
        }

        assert!(
            !token.is_cancelled(),
            "injecting must not cancel the thread"
        );
        assert!(
            threads.lock().unwrap().contains_key(&tid),
            "thread must remain active"
        );
    }

    #[test]
    fn inject_prompt_preserves_event_id() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let eid = Uuid::new_v4();
        let (_token, mut rx, _guard) = register(&threads, tid);

        {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "fix".into(),
                    event_id: Some(eid),
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
        }

        let prompt = rx.try_recv().unwrap();
        assert_eq!(prompt.text, "fix");
        assert_eq!(prompt.event_id, Some(eid));
    }

    #[test]
    fn inject_prompt_preserves_system_source() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, mut rx, _guard) = register(&threads, tid);

        {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "[Child thread completed] some task".into(),
                    event_id: None,
                    mode: thread_events::ActorMode::Agent,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
        }

        let prompt = rx.try_recv().unwrap();
        assert_eq!(prompt.mode, thread_events::ActorMode::Agent);
        assert!(prompt.text.contains("Child thread completed"));
    }

    // --- Orphaned injection tests ---

    #[test]
    fn orphaned_injection_is_recovered_by_drain() {
        // Bug: when a follow-up arrives via inject_prompt() after the agentic
        // loop's last try_recv but before the ThreadGuard drops, the message
        // sits in injection_rx and is silently lost when the function returns.
        //
        // This test verifies drain_orphaned_injections() recovers the message.
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, mut injection_rx, _guard) = register(&threads, tid);

        // Simulate: agentic loop has finished (ResponseGenerated emitted).
        // A user's follow-up arrives via inject_prompt while thread is still
        // active (guard alive, thread in active_threads).
        {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "follow-up message".to_string(),
                    event_id: Some(Uuid::new_v4()),
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
        }

        let orphans = LucidosEngine::drain_orphaned_injections(&mut injection_rx);
        assert_eq!(
            orphans.len(),
            1,
            "orphaned injection must be recovered, not silently lost"
        );
        assert_eq!(orphans[0].text, "follow-up message");
    }

    #[test]
    fn drain_orphaned_injections_returns_empty_when_no_orphans() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, mut injection_rx, _guard) = register(&threads, tid);

        let orphans = LucidosEngine::drain_orphaned_injections(&mut injection_rx);
        assert!(orphans.is_empty(), "no orphans when nothing was injected");
    }

    #[test]
    fn drain_orphaned_injections_recovers_multiple_in_order() {
        let threads = make_threads();
        let tid = Uuid::new_v4();
        let (_token, mut injection_rx, _guard) = register(&threads, tid);

        {
            let map = threads.lock().unwrap();
            let handle = map.get(&tid).unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "first follow-up".to_string(),
                    event_id: None,
                    mode: thread_events::ActorMode::Human,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
            handle
                .injection_tx
                .send(InjectedPrompt {
                    text: "second follow-up".to_string(),
                    event_id: None,
                    mode: thread_events::ActorMode::Agent,
                    spawning_event_id: None,
                    images: None,
                    origin: None,
                })
                .unwrap();
        }

        let orphans = LucidosEngine::drain_orphaned_injections(&mut injection_rx);
        assert_eq!(orphans.len(), 2);
        assert_eq!(orphans[0].text, "first follow-up");
        assert_eq!(orphans[1].text, "second follow-up");
        // Verify mode is preserved
        assert_eq!(orphans[0].mode, thread_events::ActorMode::Human);
        assert_eq!(orphans[1].mode, thread_events::ActorMode::Agent);
    }

    // --- CC exit / follow-up tests ---

    /// With no idle waiting loop, when CC exits the ThreadGuard drops immediately,
    /// allowing follow-up requests to register without any cancel hack.
    #[tokio::test]
    async fn cc_exit_drops_guard_allows_follow_up() {
        let threads = make_threads();
        let completions = make_completions();
        let tid = Uuid::new_v4();

        // 1. Simulate CC holding the thread
        {
            let (_token, _rx, _guard) = register(&threads, tid);
            assert!(threads.lock().unwrap().contains_key(&tid));
            // CC exits — guard drops here
        }

        // 2. Thread should be free immediately
        assert!(!threads.lock().unwrap().contains_key(&tid));

        // 3. Follow-up can register without any cancel hack
        let (new_token, _new_rx, _new_guard) = register_queued(&threads, &completions, tid).await;
        assert!(!new_token.is_cancelled());
    }

    #[test]
    fn partition_chat_thread_ids_excludes_idle_cc_session() {
        use std::collections::HashSet;
        let chat_only = Uuid::new_v4();
        let cc_in_flight = Uuid::new_v4();
        let cc_idle = Uuid::new_v4();
        let processing = vec![chat_only, cc_in_flight, cc_idle];
        let all_cc: HashSet<Uuid> = [cc_in_flight, cc_idle].into_iter().collect();

        let chat = partition_chat_thread_ids(&processing, &all_cc);

        assert_eq!(chat, vec![chat_only]);
    }

    #[test]
    fn partition_chat_thread_ids_keeps_chat_threads() {
        use std::collections::HashSet;
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let processing = vec![a, b];
        let all_cc: HashSet<Uuid> = HashSet::new();

        let chat = partition_chat_thread_ids(&processing, &all_cc);

        assert_eq!(chat.len(), 2);
        assert!(chat.contains(&a));
        assert!(chat.contains(&b));
    }
}
