use super::{CcCommandsResult, LucidosEngine};
use crate::engine::thread_events::{ActorMode, EngineReason, MessageOrigin};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

// NOTE: Editing this list does NOT update existing users — `cc_allowed_tools`
// seeds `~/.lucidos/cc-allowed-tools` on first run and reads from the file
// thereafter. To roll out a new entry, also append it to your own
// `~/.lucidos/cc-allowed-tools` (and tell users to do the same) — otherwise
// they keep their old seed and the CLI flag still excludes the new tool.
const DEFAULT_CC_ALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "Skill(superpowers:*)",
    "Skill(superpowers-chrome:*)",
    "AskUserQuestion",
];

const CC_ALLOWED_TOOLS_FILE: &str = "cc-allowed-tools";

/// Resolve the comma-separated tool allowlist for `claude --allowedTools`.
///
/// Reads `<user_dir>/cc-allowed-tools` if present (one entry per line, blank
/// lines and `#` comments ignored). On first call, seeds the file with the
/// compiled-in default so the user has something to discover and edit.
/// Falls back to the default if `user_dir` is `None` or any IO fails.
pub(crate) fn cc_allowed_tools(user_dir: Option<&Path>) -> String {
    let default = || DEFAULT_CC_ALLOWED_TOOLS.join(",");
    let Some(dir) = user_dir else {
        return default();
    };
    let path = dir.join(CC_ALLOWED_TOOLS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(contents) => contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let seeded = format!(
                "# One pattern per line. Lines starting with '#' are ignored.\n{}\n",
                DEFAULT_CC_ALLOWED_TOOLS.join("\n"),
            );
            if let Err(e) =
                std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, &seeded))
            {
                log!("[ClaudeCode] Failed to seed {}: {}", path.display(), e);
            }
            default()
        }
        Err(e) => {
            log!(
                "[ClaudeCode] Failed to read {}: {} — using compiled default",
                path.display(),
                e
            );
            default()
        }
    }
}

pub(crate) const AUTO_HARDEN_MESSAGE: &str = "Run /harden now.";

/// Sentinel error message returned when a CC resume attempt produces an empty
/// Result immediately — the session was stale (e.g. expired after idle timeout).
/// Callers should retry with a fresh session.
pub(crate) const STALE_RESUME_ERROR: &str = "CC_STALE_RESUME";

/// Marker file written to each CC worktree identifying the owning workspace.
pub(crate) const WORKTREE_WORKSPACE_MARKER: &str = ".lucidos-workspace";

use super::agent_session::build_merge_prompt;
use super::change_ops::{CodingAgent, LiveSessionInfo};

/// Parameters for spawning a new CC thread.
///
/// `caller_title` — if Some(non-empty), used as the thread title and LLM
/// title generation is skipped. If None, a truncated-prompt placeholder is
/// emitted and an LLM-generated title replaces it asynchronously.
pub(crate) struct SpawnAgentThreadParams {
    pub prompt: String,
    pub user_images: Option<Vec<crate::api::ChatImage>>,
    pub device_id: Option<String>,
    pub parent_thread_id: Option<Uuid>,
    pub spawning_event_id: Option<Uuid>,
    pub repo_id: Option<String>,
    pub caller_title: Option<String>,
}

impl CodingAgent for LucidosEngine {
    async fn is_running_for(&self, thread_id: Uuid) -> bool {
        self.is_agent_running_for(thread_id).await
    }

    fn spawn_hardening(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        self.spawn_hardening_session(
            thread_id,
            worktree_path,
            branch_name,
            auto_apply_change_id,
            actor,
        );
    }

    async fn live_session_info(&self, thread_id: Uuid) -> Option<LiveSessionInfo> {
        let guard = self.agent_sessions.lock().await;
        guard.get(&thread_id).and_then(|s| {
            if s.process_exited {
                return None;
            }
            Some(LiveSessionInfo {
                worktree_path: s.worktree_path.clone()?,
                idle_notify: s.idle_notify.clone(),
                msg_tx: s.msg_tx.clone(),
            })
        })
    }

    async fn merge_via_session(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        session: &LiveSessionInfo,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        self.merge_via_cc_session(
            thread_id,
            change_id,
            wt_path,
            branch_name,
            repo_root,
            &session.idle_notify,
            &session.msg_tx,
        )
        .await
    }

    fn spawn_merge_session(&self, thread_id: Uuid, change_id: Uuid, description: &str) {
        let engine = self.clone_arc();
        let description = description.to_string();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let change = match engine.changes().get_by_id(change_id).await {
                Some(c) => c,
                None => {
                    log!(
                        "[MergeConflict] Change {} not found for spawn_merge_session",
                        change_id
                    );
                    return;
                }
            };

            // `files` is the change's full file list, not necessarily
            // conflicting — Tier 2 has no worktree yet to probe.
            engine
                .emit_merge_conflict_detected(thread_id, change_id, change.files.clone())
                .await;

            let prompt = build_merge_prompt(
                &change.branch_name,
                Some("You are running in a temporary merge worktree."),
                Some(&description),
            );

            let origin_id = match engine.emit_automated_prompt(thread_id, &prompt, None).await {
                Ok(id) => id,
                Err(e) => {
                    log!(
                        "[MergeConflict] Failed to emit merge prompt for thread {}: {}",
                        thread_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Failed to emit prompt: {}", e),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[MergeConflict] ResponseFailed",
                        )
                        .await;
                    return;
                }
            };

            let request_id = Uuid::new_v4();
            let cancel_token = tokio_util::sync::CancellationToken::new();
            let result = engine
                .run_direct_agent(
                    request_id,
                    thread_id,
                    &prompt,
                    None,
                    origin_id,
                    None,
                    &cancel_token,
                    Some(change_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
            match result {
                Ok(_) => {
                    log!(
                        "[MergeConflict] CC session completed for change {}",
                        change_id
                    );
                    engine.broadcast_changes_updated().await;
                }
                Err(e) => {
                    log!(
                        "[MergeConflict] CC session failed for change {}: {}",
                        change_id,
                        e
                    );
                    emit_background_task_failure(
                        &engine,
                        thread_id,
                        &e,
                        "[MergeConflict] spawn_merge_session failure",
                    )
                    .await;
                }
            }
        });
    }

    async fn lookup_session_id_for_resume(&self, thread_id: Uuid) -> Option<String> {
        sqlx::query_scalar(
            "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await
        .unwrap_or(None)
    }

    async fn run_merge_session_tier2(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        description: &str,
        resume_token: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let merge_prompt = build_merge_prompt("main", None, Some(description));
        let origin_id = self
            .emit_automated_prompt(thread_id, &merge_prompt, None)
            .await?;
        let request_id = Uuid::new_v4();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        self.run_direct_agent(
            request_id,
            thread_id,
            &merge_prompt,
            None,
            origin_id,
            None,
            &cancel_token,
            Some(change_id),
            Some((wt_path.to_path_buf(), branch_name.to_string())),
            None,
            None,
            resume_token,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }
}

/// Returns true iff `thread_summaries.status` is currently `'running'`.
/// Best-effort gate — caller and a concurrent settler can still both pass
/// this check before either's emit lands; treat as a duplicate-reducer,
/// not a guarantee.
pub(crate) async fn thread_is_running(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(status.as_deref()
        == Some(crate::engine::thread_lifecycle::ThreadStatus::Running.as_str()))
}

/// Emit `ResponseFailed` for a background task that errored, but only if the
/// projection still shows the thread as `running`. Prevents double-terminal
/// in the common case where Stop/Discard already settled the thread; see
/// `thread_is_running` for the race caveat.
pub(crate) async fn emit_background_task_failure(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    error: impl std::fmt::Display,
    label: &str,
) {
    if thread_is_running(engine.pool(), thread_id)
        .await
        .unwrap_or(false)
    {
        engine
            .event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                        error: error.to_string(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                label,
            )
            .await;
    }
}

/// Emit a terminal `ResponseAborted` event for a thread that the projection
/// still considers `running` even though no live agent session or in-process
/// agentic loop is driving it. This is the safety net for stuck threads
/// (e.g. a `spawn_agent_thread` task that errored before it could emit a
/// terminal event, or any orphan left over from a prior engine instance).
///
/// Returns `Ok(true)` if a `ResponseAborted` was emitted, `Ok(false)` if
/// the thread was already settled (or doesn't exist).
pub(crate) async fn settle_stuck_running_thread(
    pool: &sqlx::PgPool,
    bus: &super::event_bus::EventBus,
    thread_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if !thread_is_running(pool, thread_id).await? {
        return Ok(false);
    }

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: crate::engine::thread_events::EventMeta::NONE,
    })
    .await?;

    Ok(true)
}

impl LucidosEngine {
    /// Check if any Claude Code session is running for a specific thread.
    pub async fn is_agent_running_for(&self, thread_id: Uuid) -> bool {
        let guard = self.agent_sessions.lock().await;
        guard
            .get(&thread_id)
            .map(|s| !s.process_exited)
            .unwrap_or(false)
    }

    /// Cancel a running Claude Code process via notify signal.
    /// If `auto_apply` is true, the resulting proposed change will be applied immediately.
    /// If `thread_id` is provided, cancel that specific session; otherwise cancel all.
    ///
    /// `actor` identifies the user who clicked Stop / Apply / Done — flows into
    /// any resulting `ChangeApplied` / `ChangeApplyFailed` events stamped via
    /// the stale-session fallback. HTTP callers build it; engine-internal
    /// shutdowns pass `None`.
    ///
    /// No-live-session fallback mirrors `interrupt_agent`: cancel the
    /// in-process token, try `end_stale_waiting_session`, then settle any
    /// stuck `running` projection so the Discard button doesn't 404 on
    /// threads whose `spawn_agent_thread` errored before SessionStarted.
    pub async fn cancel_agent(
        self: &Arc<Self>,
        auto_apply: bool,
        discard: bool,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get_mut(&tid) {
                session.auto_apply = auto_apply;
                session.discard = discard;
                session.cancel.notify_one();
                Ok(())
            } else {
                drop(guard);
                self.cancel_thread(tid);
                let stale_result = self
                    .end_stale_waiting_session(tid, auto_apply, discard, actor)
                    .await;
                let settled = settle_stuck_running_thread(&self.pool, &self.event_bus, tid)
                    .await
                    .unwrap_or_else(|e| {
                        log!(
                            "[ClaudeCode] settle_stuck_running_thread failed for {}: {}",
                            tid,
                            e
                        );
                        false
                    });
                if settled {
                    Ok(())
                } else {
                    stale_result
                }
            }
        } else {
            if guard.is_empty() {
                return Err("No Claude Code process is running".into());
            }
            for session in guard.values_mut() {
                session.auto_apply = auto_apply;
                session.discard = discard;
                session.cancel.notify_one();
            }
            Ok(())
        }
    }

    /// Interrupt a running Claude Code session — sends control_request:interrupt to stop
    /// current work without killing the session (like pressing Esc in the CC terminal).
    /// The CC process stays alive and enters waiting state.
    ///
    /// Fallbacks (in order) when no live CC subprocess exists for `thread_id`:
    ///   1. Cancel the in-process agentic loop via the `active_threads` token
    ///      (handles run_thread-spawned chat threads and CC threads that are
    ///      mid-startup, before SessionStarted has registered an agent_session).
    ///   2. If the projection still shows the thread as `running` (no terminal
    ///      event will ever arrive — e.g. the spawn task errored and was lost),
    ///      emit a `ResponseAborted` so the UI unsticks. Without this, the
    ///      stop button on a stuck thread would 404 forever.
    pub async fn interrupt_agent(
        &self,
        thread_id: Option<Uuid>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get(&tid) {
                if session.is_waiting {
                    return Err("Session is already waiting".into());
                }
                session.interrupt.notify_one();
                Ok(())
            } else {
                drop(guard);
                self.cancel_thread(tid);
                if let Err(e) =
                    settle_stuck_running_thread(&self.pool, &self.event_bus, tid).await
                {
                    log!(
                        "[ClaudeCode] settle_stuck_running_thread failed for {}: {}",
                        tid,
                        e
                    );
                }
                Ok(())
            }
        } else {
            if guard.is_empty() {
                return Err("No Claude Code process is running".into());
            }
            for session in guard.values() {
                if !session.is_waiting {
                    session.interrupt.notify_one();
                }
            }
            Ok(())
        }
    }

    /// Send a control request to a running Claude Code session via its control channel.
    pub async fn send_agent_control_request(
        &self,
        thread_id: Uuid,
        request: crate::runtime::ControlRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event = crate::engine::thread_events::ThreadEvent::from_control_request(
            &request,
            crate::runtime::AgentKind::ClaudeCode,
        );
        {
            let mut guard = self.agent_sessions.lock().await;
            let session = guard
                .get_mut(&thread_id)
                .ok_or("No Claude Code session found for this thread")?;
            if let crate::runtime::ControlRequest::SetModel { ref model } = request {
                session.current_model = Some(model.clone());
            }
            if let crate::runtime::ControlRequest::SetReasoningEffort { ref effort } = request {
                session.current_reasoning_effort = Some(effort.clone());
            }
            session
                .control_tx
                .send(request)
                .map_err(|_| "Control channel closed")?;
        }
        if let Some(ev) = event {
            if let Err(e) = self
                .event_bus
                .emit(crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: ev,
                    meta: crate::engine::thread_events::EventMeta {
                        channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                })
                .await
            {
                log!(
                    "[ClaudeCode] Failed to persist CodingAgentSettingsChanged for {}: {}",
                    thread_id,
                    e
                );
            }
        }
        Ok(())
    }

    /// Query the latest CC settings (model, reasoning_effort) for a thread from events.
    /// Returns (model, effort) — each is None if never changed.
    pub(crate) async fn cc_thread_settings(
        &self,
        thread_id: Uuid,
    ) -> (Option<String>, Option<String>) {
        let tid = thread_id.to_string();
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
             ORDER BY created DESC LIMIT 10",
        )
        .bind(&tid)
        .fetch_all(self.pool())
        .await
        .unwrap_or_default();
        // Fold from newest to oldest — first non-null value wins for each field
        let mut model: Option<String> = None;
        let mut effort: Option<String> = None;
        for (payload,) in &rows {
            if model.is_none() {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    model = Some(m.to_string());
                }
            }
            if effort.is_none() {
                if let Some(e) = payload.get("reasoning_effort").and_then(|v| v.as_str()) {
                    effort = Some(e.to_string());
                }
            }
            if model.is_some() && effort.is_some() {
                break;
            }
        }
        (model, effort)
    }

    pub async fn cc_categorized_commands(&self, thread_id: Uuid) -> CcCommandsResult {
        let guard = self.agent_sessions.lock().await;
        if let Some(s) = guard.get(&thread_id) {
            let has_active = !s.process_exited;
            let mut info = s.to_commands_info();
            let model = s.current_model.clone();
            let effort = s.current_reasoning_effort.clone();
            // Session exists but Init hasn't arrived yet — use repo cache for commands
            if info.builtin_commands.is_empty() && info.skill_commands.is_empty() {
                if let Some(ref repo) = s.repo_root {
                    let repo_key = repo.to_string_lossy().to_string();
                    drop(guard);
                    let cache = self.cc_commands_cache.read().await;
                    if let Some(cached) = cache.get(&repo_key) {
                        info.builtin_commands = cached.builtin_commands.clone();
                        info.skill_commands = cached.skill_commands.clone();
                    }
                }
            }
            return CcCommandsResult {
                info,
                has_active_session: has_active,
                current_model: model,
                current_reasoning_effort: effort,
            };
        }
        drop(guard);
        // No live session — get commands from repo cache, settings from thread events
        let repo_root: Option<String> = sqlx::query_scalar(
            "SELECT repo_root FROM changes WHERE thread_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await
        .map_err(|e| {
            log!(
                "[ClaudeCode] Failed to look up repo_root for {}: {}",
                thread_id,
                e
            );
            e
        })
        .ok()
        .flatten();
        let commands = {
            let cache = self.cc_commands_cache.read().await;
            if let Some(repo_key) = repo_root {
                cache.get(&repo_key).cloned()
            } else {
                cache.values().next().cloned()
            }
        };
        let (model, effort) = self.cc_thread_settings(thread_id).await;
        CcCommandsResult {
            info: commands.unwrap_or_default(),
            has_active_session: false,
            current_model: model,
            current_reasoning_effort: effort,
        }
    }

    /// Return cached commands without needing a thread (for compose-view menu).
    pub async fn cc_cached_commands(&self) -> CcCommandsResult {
        let cache = self.cc_commands_cache.read().await;
        CcCommandsResult {
            info: cache.values().next().cloned().unwrap_or_default(),
            has_active_session: false,
            current_model: None,
            current_reasoning_effort: None,
        }
    }

    // end_stale_waiting_session, recover_orphaned_worktrees → moved to agent_recovery.rs

    /// Spawn a CC subprocess to run `/harden` on the given branch's worktree.
    /// When `auto_apply_change_id` is `Some`, the apply is re-entered after
    /// hardening completes (or `ChangeApplyFailed` is emitted on failure).
    /// `actor` carries the user who initiated the original apply so the
    /// resulting `ChangeApplied` stamps that user instead of the engine
    /// fallback.
    pub(crate) fn spawn_hardening_session(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone_arc();
        Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
            let prompt = "Your changes have not been hardened. Run /harden now.";

            // Engine-retriggered harden: stamp with the dedicated reason so the
            // route popover surfaces "Engine · Harden auto-retrigger" instead of
            // "Unknown".
            let origin = Some(MessageOrigin::engine(EngineReason::HardenRetrigger));
            let origin_id = match engine.emit_automated_prompt(thread_id, prompt, origin).await {
                Ok(id) => id,
                Err(e) => {
                    log!(
                        "[ClaudeCode] Failed to emit hardening prompt for thread {}: {}",
                        thread_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                                    error: format!("Failed to emit prompt: {}", e),
                                },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] ResponseFailed",
                        )
                        .await;
                    return;
                }
            };

            let request_id = Uuid::new_v4();
            let cancel_token = tokio_util::sync::CancellationToken::new();

            let hardening_system_prompt = format!(
                "HARDENING SESSION: You are hardening changes on branch `{}`. \
                 Your ONLY task is to run the /harden skill. Do NOT continue any implementation work, \
                 do NOT look at implementation plans, do NOT add features. \
                 Just harden the existing changes for quality and correctness.\n\n\
                 CRITICAL: Never run `exit` as a bash command.",
                branch_name
            );
            let result = engine
                .run_direct_agent(
                    request_id,
                    thread_id,
                    prompt,
                    None,
                    origin_id,
                    None,
                    &cancel_token,
                    None,
                    Some((worktree_path, branch_name)), // recovery_worktree — reuse existing worktree
                    None,
                    Some(hardening_system_prompt),
                    None,
                    None,
                    None,
                    None,
                )
                .await;

            let Some(change_id) = auto_apply_change_id else {
                match result {
                    Ok(_) => log!(
                        "[ClaudeCode] Hardening session completed for thread {}",
                        thread_id
                    ),
                    Err(e) => log!(
                        "[ClaudeCode] Hardening session failed for thread {}: {}",
                        thread_id,
                        e
                    ),
                }
                return;
            };

            match result {
                Ok(_) => {
                    log!(
                        "[ClaudeCode] Hardening completed for change {}, auto-applying",
                        change_id
                    );
                    // Emit ChangeHardened — projection updates from the event so
                    // apply_change won't re-enter the unhardened path and spawn
                    // another hardening → infinite loop.
                    engine
                        .emit_change_hardened(thread_id, change_id, "[ClaudeCode] ChangeHardened")
                        .await;
                    match engine.apply_change(change_id, actor.clone()).await {
                        Ok(r) => {
                            log!(
                                "[ClaudeCode] Auto-applied change {} after hardening: {}",
                                change_id,
                                r.message
                            );
                            engine.broadcast_changes_updated().await;
                        }
                        Err(e) => {
                            log!(
                                "[ClaudeCode] Auto-apply failed after hardening for change {}: {}",
                                change_id,
                                e
                            );
                            engine
                                .event_bus
                                .emit_or_log(
                                    crate::engine::event_bus::BusEvent::Thread {
                                        thread_id,
                                        event:
                                            crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                                change_id: change_id.to_string(),
                                                error: e.to_string(),
                                                actor: actor.clone(),
                                            },
                                        meta: crate::engine::thread_events::EventMeta::NONE,
                                    },
                                    "[ClaudeCode] ChangeApplyFailed",
                                )
                                .await;
                        }
                    }
                }
                Err(e) => {
                    log!(
                        "[ClaudeCode] Hardening session failed for change {}: {}",
                        change_id,
                        e
                    );
                    engine
                        .event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                        change_id: change_id.to_string(),
                                        error: format!("Hardening failed: {}", e),
                                        actor: actor.clone(),
                                    },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[ClaudeCode] ChangeApplyFailed (hardening)",
                        )
                        .await;
                }
            }
        });
    }

    // apply_now, wait_for_idle, send_and_wait, apply_now_inner, merge_via_cc_session,
    // apply_now_success, reset_worktree_and_idle, kill_cc_and_flush, make_terminal_event,
    // emit_automated_prompt, spawn_cc_task_guarded, monitor_cc_task, run_direct_agent
    // → moved to agent_session.rs

    /// End a CC session for a thread — kill process, clean up worktree, emit SessionEnded.
    /// Called when user clicks Done on a CC thread.
    /// Passes discard=true so any remaining worktree changes are discarded rather
    /// than proposed — the user said "Done", so leftover changes shouldn't
    /// re-activate the thread with a waiting dot.
    ///
    /// `actor` is the user who clicked Done — propagated so any
    /// `ChangeApplyFailed` emitted by the stale-session fallback carries the
    /// real user instead of falling back to the engine chip.
    pub async fn end_cc_session_for_thread(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // cancel_agent handles both live sessions (cancel.notify_one)
        // and stale sessions (end_stale_waiting_session) internally
        self.cancel_agent(false, true, Some(thread_id), actor).await
    }

    /// Discard pending CC changes without ending the session.
    /// Resets the worktree to main and re-enters idle state.
    ///
    /// `actor` is the user who clicked Discard — propagated to any
    /// `ChangeApplyFailed` emitted by the stale-session fallback so the
    /// resulting event carries the real actor.
    pub async fn discard_cc_changes(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let wt = {
            let guard = self.agent_sessions.lock().await;
            if let Some(session) = guard.get(&thread_id) {
                session
                    .worktree_path
                    .clone()
                    .ok_or("No worktree for this session")?
            } else {
                // No live session — fall back to stale session handling
                return self
                    .end_stale_waiting_session(thread_id, false, true, actor)
                    .await;
            }
        };

        self.discard_pending_for_thread(thread_id).await;

        self.reset_worktree_and_idle(thread_id, &wt).await;

        self.broadcast_changes_updated().await;

        Ok(())
    }

    /// Spawn a Claude Code session in a new independent thread.
    ///
    /// Creates a new thread_id, persists a MessageReceived event, sets a title
    /// (caller-provided or generated), and spawns CC in a background tokio task.
    /// Returns the new thread_id immediately.
    pub(crate) async fn spawn_agent_thread(
        &self,
        params: SpawnAgentThreadParams,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let SpawnAgentThreadParams {
            prompt,
            user_images,
            device_id,
            parent_thread_id,
            spawning_event_id,
            repo_id,
            caller_title,
        } = params;

        let cc_thread_id = Uuid::new_v4();
        let explicit_title = caller_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        let has_explicit_title = explicit_title.is_some();
        let title = explicit_title.unwrap_or_else(|| prompt.chars().take(60).collect::<String>());

        let device_name = if let Some(ref did) = device_id {
            crate::core::DeviceStore::display_name(&self.pool, did).await
        } else {
            None
        };

        // Persist + broadcast MessageReceived for the CC thread.
        // Route through the canonical synthesizer so `mode + parent_thread_id`
        // produces a `ParentThread { mode: Agent }` origin — without it, the
        // frontend's `originMode(undefined)` defaults to `engine` and the chip
        // mislabels the spawn as engine-initiated work.
        let emit_result = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: cc_thread_id,
                event: crate::engine::chat::make_message_received(
                    &prompt,
                    user_images.as_deref(),
                    device_id.as_deref(),
                    device_name,
                    parent_thread_id,
                    spawning_event_id,
                    ActorMode::Agent,
                    None,
                    None,
                    None,
                ),
                meta: crate::engine::thread_events::EventMeta {
                    channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            })
            .await?
            .expect("persisted event must return EmitResult");
        let origin_id = emit_result.event_id;

        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: cc_thread_id,
                event: crate::engine::thread_events::ThreadEvent::ThreadTitleGenerated {
                    title: title.clone(),
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
        {
            log!("[ClaudeCode] Failed to emit title: {}", e);
        }

        if !has_explicit_title {
            if let Some(ref extractor) = self.extractor {
                let title_model =
                    crate::core::PreferenceStore::get(&self.pool, crate::core::PREF_MODEL_TITLE)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                let provider = extractor.provider_for_model(&title_model);
                let msg = prompt.clone();
                let bus = self.event_bus.clone();
                tokio::spawn(async move {
                    super::chat::emit_generated_title(
                        &bus,
                        &provider,
                        cc_thread_id,
                        &msg,
                        None,
                        None,
                    )
                    .await;
                });
            }
        }

        // Notify parent thread about the new CC thread via EventBus
        // CodingAgentThreadSpawned is transient — just tells the frontend a new thread was spawned
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id: cc_thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentThreadSpawned {
                        cc_thread_id: cc_thread_id.to_string(),
                        title: title.clone(),
                        agent: crate::runtime::AgentKind::ClaudeCode,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[ClaudeCode] CodingAgentThreadSpawned",
            )
            .await;

        // Register the CC thread as active BEFORE spawning the background task.
        // This ensures list_threads API includes it in the active set immediately,
        // preventing a race where loadAllThreads sees the thread (from persisted
        // events) but not in the active set, causing it to show in History.
        let (cancel_token, _injection_rx, guard) = self.register_thread(cc_thread_id);

        // Spawn CC in a background task
        let engine = self.clone_arc();
        let images = user_images;

        let handle = tokio::spawn(async move {
            // Guard moved here — auto-unregisters on drop when task ends
            let _guard = guard;

            // All events flow through EventBus — no forwarder needed
            let result = engine
                .run_direct_agent(
                    cc_thread_id,
                    cc_thread_id,
                    &prompt,
                    images.as_deref(),
                    origin_id,
                    spawning_event_id,
                    &cancel_token,
                    None,
                    None,
                    repo_id,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;

            match result {
                Ok(ref res) => {
                    if res.proposed_change {
                        if res.auto_apply {
                            let pending = engine.changes().list_pending().await;
                            if let Some(change) =
                                pending.iter().find(|c| c.request_id == res.request_id)
                            {
                                match engine.apply_change(change.id, None).await {
                                    Ok(r) => {
                                        log!("[ClaudeCode] Auto-applied change: {}", r.message)
                                    }
                                    Err(e) => {
                                        log!("[ClaudeCode] Failed to auto-apply: {}", e);
                                        engine
                                            .event_bus
                                            .emit_or_log(
                                                crate::engine::event_bus::BusEvent::Thread {
                                                    thread_id: cc_thread_id,
                                                    event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                                        change_id: change.id.to_string(),
                                                        error: e.to_string(),
                                                        actor: None,
                                                    },
                                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                                },
                                                "[ClaudeCode] ChangeApplyFailed",
                                            )
                                            .await;
                                    }
                                }
                            }
                        }

                        engine.broadcast_changes_updated().await;
                    }
                }
                Err(e) => {
                    log!("[ClaudeCode] Background CC session failed: {}", e);
                    emit_background_task_failure(
                        &engine,
                        cc_thread_id,
                        &e,
                        "[ClaudeCode] spawn_agent_thread failure",
                    )
                    .await;
                }
            }
        });

        Self::monitor_cc_task(self.clone_arc(), cc_thread_id, handle);

        Ok(cc_thread_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{cc_allowed_tools, settle_stuck_running_thread, DEFAULT_CC_ALLOWED_TOOLS};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    /// Emit MessageReceived for a CC-channel thread → status='running'.
    /// Mirrors what `spawn_agent_thread` does before kicking off the bg task.
    async fn seed_running_cc_thread(bus: &EventBus, thread_id: Uuid) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "do the thing".into(),
                images: vec![],
                device_id: None,
                device: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: ActorMode::Agent,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    async fn read_status(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    /// Reproduces the user-reported bug: a CC thread is stuck at status='running'
    /// because its background spawn task errored before any terminal event
    /// could be emitted. The settle helper unsticks it.
    #[tokio::test]
    async fn settle_stuck_running_thread_emits_aborted_and_transitions_to_failed() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_running_cc_thread(&bus, thread_id).await;
        assert_eq!(read_status(&pool, thread_id).await.as_deref(), Some("running"));

        let did_emit = settle_stuck_running_thread(&pool, &bus, thread_id)
            .await
            .unwrap();
        assert!(did_emit, "stuck running thread should be settled");
        assert_eq!(
            read_status(&pool, thread_id).await.as_deref(),
            Some("failed"),
            "ResponseAborted on a 'running' thread must transition status → 'failed'",
        );

        // Verify the actual ResponseAborted event was persisted, not some other event.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'"
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "exactly one ResponseAborted should be persisted");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Idempotency: settling a thread that's already non-running is a no-op
    /// (so that double-clicks on the stop button don't pile up events).
    #[tokio::test]
    async fn settle_stuck_running_thread_no_op_when_already_settled() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        seed_running_cc_thread(&bus, thread_id).await;
        // First settle transitions running → failed.
        assert!(settle_stuck_running_thread(&pool, &bus, thread_id)
            .await
            .unwrap());
        // Second settle should be a no-op.
        assert!(!settle_stuck_running_thread(&pool, &bus, thread_id)
            .await
            .unwrap());

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'"
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "second settle must not emit a duplicate event");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A thread that the projection never knew about (no thread_summaries row)
    /// is also a no-op — interrupt of an unknown id should not emit phantom
    /// events for non-existent threads.
    #[tokio::test]
    async fn settle_stuck_running_thread_no_op_for_unknown_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        let did_emit = settle_stuck_running_thread(&pool, &bus, Uuid::new_v4())
            .await
            .unwrap();
        assert!(!did_emit);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn cc_allowed_tools_returns_default_when_user_dir_missing() {
        assert_eq!(cc_allowed_tools(None), DEFAULT_CC_ALLOWED_TOOLS.join(","));
    }

    #[test]
    fn cc_allowed_tools_seeds_default_file_on_first_use() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cc-allowed-tools");
        assert!(!path.exists());

        let result = cc_allowed_tools(Some(dir.path()));

        assert_eq!(result, DEFAULT_CC_ALLOWED_TOOLS.join(","));
        assert!(path.exists(), "seed file should have been written");
        let seeded = std::fs::read_to_string(&path).unwrap();
        assert!(seeded.contains("Bash"));
        assert!(seeded.starts_with("# "));
    }

    #[test]
    fn cc_allowed_tools_parses_user_file_strips_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cc-allowed-tools"),
            "# header comment\n\nBash\nRead\n  Edit  \n# inline comment\nSkill(superpowers:*)\n\n",
        )
        .unwrap();

        assert_eq!(
            cc_allowed_tools(Some(dir.path())),
            "Bash,Read,Edit,Skill(superpowers:*)",
        );
    }

    /// Validates that idle_notify (using notify_waiters) wakes a registered waiter.
    /// This is the contract that send_and_wait and apply_now_inner depend on:
    /// the Result handler must call idle_notify.notify_waiters() so that
    /// any task waiting on idle_notify.notified() wakes up.
    #[tokio::test]
    async fn idle_notify_wakes_registered_waiter() {
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify2 = notify.clone();

        // Simulate send_and_wait: register a waiter BEFORE the notification
        let waiter = tokio::spawn(async move {
            // Use the same 5s timeout pattern as send_and_wait
            match tokio::time::timeout(std::time::Duration::from_secs(2), notify2.notified()).await
            {
                Ok(()) => true,  // Woken by notify_waiters
                Err(_) => false, // Timed out — notify_waiters was never called
            }
        });

        // Give the waiter time to register
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Simulate the Result handler firing idle_notify
        notify.notify_waiters();

        let woken = waiter.await.unwrap();
        assert!(
            woken,
            "idle_notify.notify_waiters() must wake registered waiters — \
                         this is the contract that send_and_wait depends on"
        );
    }

    /// Validates that notify_waiters does NOT store a permit — calling it
    /// BEFORE a waiter registers means the waiter misses the notification.
    /// This is why bare `idle_notify.notified().await` (without a poll loop)
    /// is dangerous: if the notification fires before the await starts, it's lost.
    #[tokio::test]
    async fn notify_waiters_does_not_store_permit() {
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());

        // Fire notification BEFORE any waiter is registered
        notify.notify_waiters();

        // Now try to wait — should NOT wake up (permit not stored)
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;

        assert!(result.is_err(), "notify_waiters() must NOT store a permit — \
                                   bare .notified().await after a missed notification hangs forever");
    }

    /// Validates the resume branch reuse logic: when a branch exists,
    /// `git worktree add` should use the existing branch (no -b flag)
    /// instead of creating a new one.
    #[tokio::test]
    async fn resume_branch_reuses_existing_branch() {
        use crate::engine::git_ops::git_cmd;

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Set up a git repo with an initial commit
        let _ = git_cmd(&["init"], repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
        let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "init"], repo).await;

        // Create a branch with a commit (simulating a previous CC session)
        let branch_name = "claude-code/20260326-test";
        let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
        let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
        let _ = git_cmd(&["checkout", "main"], repo).await;

        // Verify the branch exists
        let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(exists, "Test branch should exist");

        // Create a worktree from the existing branch (no -b flag)
        let wt_path = tmp.path().join("worktree-resume");
        let result = git_cmd(
            &["worktree", "add", wt_path.to_str().unwrap(), branch_name],
            repo,
        )
        .await;
        assert!(
            result.unwrap().status.success(),
            "Should create worktree from existing branch"
        );

        // The worktree should have the CC changes
        let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
            .await
            .unwrap();
        assert_eq!(
            content, "cc changes",
            "Resumed worktree should contain previous CC changes"
        );

        // Clean up
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            repo,
        )
        .await;
    }

    /// When a worktree already exists for the resume branch (e.g. left over from
    /// a previous engine session), `parse_worktree_list` should detect it so the
    /// caller can reuse the existing worktree instead of failing with
    /// "branch is already used by worktree at ...".
    #[tokio::test]
    async fn resume_reuses_existing_worktree_for_branch() {
        use crate::engine::git_ops::{git_cmd, parse_worktree_list};

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Set up a git repo with an initial commit
        let _ = git_cmd(&["init"], repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
        let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "init"], repo).await;

        // Create a branch and a worktree for it (simulating a previous CC session)
        let branch_name = "claude-code/20260326-leftover";
        let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
        let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
        let _ = git_cmd(&["checkout", "main"], repo).await;

        let wt_path = tmp.path().join("old-worktree");
        let result = git_cmd(
            &["worktree", "add", wt_path.to_str().unwrap(), branch_name],
            repo,
        )
        .await;
        assert!(
            result.unwrap().status.success(),
            "Setup: should create initial worktree"
        );

        // Now verify that parse_worktree_list detects the existing worktree
        let list_output = git_cmd(&["worktree", "list", "--porcelain"], repo)
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&list_output.stdout);
        let map = parse_worktree_list(&stdout);
        let found = map.get(branch_name);
        assert!(
            found.is_some(),
            "parse_worktree_list should find the existing worktree for branch {}",
            branch_name
        );

        // Verify it points to the correct path
        let found_path = found.unwrap();
        let canonical_expected = wt_path.canonicalize().unwrap();
        let canonical_found = found_path.canonicalize().unwrap();
        assert_eq!(
            canonical_found,
            canonical_expected,
            "Existing worktree path should match: expected {}, got {}",
            canonical_expected.display(),
            canonical_found.display()
        );

        // Trying to create a SECOND worktree for the same branch should fail
        let wt_path_new = tmp.path().join("new-worktree");
        let fail_result = git_cmd(
            &[
                "worktree",
                "add",
                wt_path_new.to_str().unwrap(),
                branch_name,
            ],
            repo,
        )
        .await;
        assert!(
            !fail_result.unwrap().status.success(),
            "git worktree add should fail when branch already checked out in another worktree"
        );

        // Verify the original worktree content is still intact
        let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
            .await
            .unwrap();
        assert_eq!(
            content, "cc changes",
            "Existing worktree should preserve CC changes"
        );

        // Clean up
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            repo,
        )
        .await;
    }

    /// When the resume branch no longer exists, a fresh branch should be created.
    #[tokio::test]
    async fn resume_falls_back_when_branch_deleted() {
        use crate::engine::git_ops::git_cmd;

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Set up a git repo
        let _ = git_cmd(&["init"], repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
        let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
        let _ = git_cmd(&["add", "."], repo).await;
        let _ = git_cmd(&["commit", "-m", "init"], repo).await;

        // Verify a non-existent branch
        let branch_name = "claude-code/20260326-deleted";
        let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(!exists, "Deleted branch should not exist");

        // The code should fall back to creating a new branch
        let new_branch = crate::engine::agent_session::generate_cc_branch_name();
        let wt_path = tmp.path().join("worktree-fresh");
        let result = git_cmd(
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                &new_branch,
            ],
            repo,
        )
        .await;
        assert!(
            result.unwrap().status.success(),
            "Should create worktree with fresh branch"
        );

        // Clean up
        let _ = git_cmd(
            &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
            repo,
        )
        .await;
    }

    /// Test helper: create a `AgentSession` with sensible defaults.
    /// Only `msg_tx` and `is_waiting` vary across tests.
    fn make_test_session(
        msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
        is_waiting: bool,
    ) -> crate::engine::AgentSession {
        use std::sync::Arc;
        crate::engine::AgentSession {
            msg_tx,
            is_waiting,
            has_changes: false,
            requires_restart: false,
            auto_apply: false,
            discard: false,
            cancel: Arc::new(tokio::sync::Notify::new()),
            interrupt: Arc::new(tokio::sync::Notify::new()),
            idle_notify: Arc::new(tokio::sync::Notify::new()),
            apply_now_in_progress: false,
            process_exited: false,
            worktree_path: None,
            branch_name: None,
            repo_root: None,
            cc_session_id: None,
            shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            control_tx: tokio::sync::mpsc::unbounded_channel().0,
            builtin_commands: vec![],
            skill_commands: vec![],
            current_model: None,
            current_reasoning_effort: None,
            last_event_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        }
    }

    /// Validates that when a CC session is idle (is_waiting=true), a follow-up
    /// message is routed via msg_tx instead of being rejected with
    /// "Claude Code is already running".
    #[tokio::test]
    async fn idle_session_routes_followup_via_msg_tx() {
        use crate::engine::AgentUserInput;

        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let session = make_test_session(msg_tx, true);

        // Simulate single-lock routing logic from run_direct_agent
        assert!(!session.process_exited);
        assert!(session.is_waiting);

        // Route via msg_tx (same as production code)
        assert!(
            session
                .msg_tx
                .send(AgentUserInput {
                    text: "Follow-up message".to_string(),
                    images: None,
                    origin_event_id: None,
                })
                .is_ok(),
            "send should succeed when receiver is alive"
        );

        let received = msg_rx
            .try_recv()
            .expect("msg_rx should have received the follow-up");
        assert_eq!(received.text, "Follow-up message");
    }

    /// Validates that when a CC session is actively working (is_waiting=false),
    /// the follow-up is rejected (not routed via msg_tx).
    #[tokio::test]
    async fn busy_session_rejects_followup() {
        use crate::engine::AgentUserInput;

        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let session = make_test_session(msg_tx, false);

        assert!(!session.process_exited);
        assert!(
            !session.is_waiting,
            "Busy session should reject follow-up with 'already running' error"
        );
    }

    /// Validates that when the msg_tx channel is closed (receiver dropped),
    /// send returns Err — production code should propagate the error.
    #[tokio::test]
    async fn closed_channel_returns_error() {
        use crate::engine::AgentUserInput;

        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        let session = make_test_session(msg_tx, true);

        // Drop the receiver to simulate CC process exit
        drop(msg_rx);

        let result = session.msg_tx.send(AgentUserInput {
            text: "too late".to_string(),
            images: None,
            origin_event_id: None,
        });
        assert!(result.is_err(), "send should fail when receiver is dropped");
    }
}
