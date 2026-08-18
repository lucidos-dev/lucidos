use super::*;
use crate::engine::CcCommandsResult;

impl LucidosEngine {
    /// Check if any Claude Code session is running for a specific thread.
    pub async fn is_agent_running_for(&self, thread_id: Uuid) -> bool {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|s| s.is_live())
    }

    /// Stop a running Claude Code session via the generic stop signal.
    /// `reason` is recorded on the session as `pending_stop` and read by the
    /// run_session loop's stop arm + post-loop cleanup — see `StopReason` for
    /// the per-variant terminator semantics.
    ///
    /// `actor` identifies the user who clicked the button. Flows into any
    /// resulting `ChangeApplied` / `ChangeApplyFailed` events stamped via
    /// the stale-session fallback. Engine-internal shutdowns pass `None`.
    ///
    /// `thread_id = None` stops every session (engine shutdown timeout path).
    ///
    /// No-live-session fallback mirrors `interrupt_agent`: cancel the
    /// in-process token, try `end_stale_waiting_session`, then settle any
    /// stuck `running` projection so the Discard button doesn't 404 on
    /// threads whose `spawn_agent_thread` errored before SessionStarted.
    ///
    /// That settle is always `SettleTerminal::StuckProjection`, unlike
    /// `interrupt_agent`'s. Apply / Discard / Archive each carry their own
    /// terminator (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`), and
    /// suppressing the otherwise-spurious `ResponseCanceled` is what
    /// [`StopReason`] exists for. So a cancel-stamped question must not produce
    /// one here.
    pub async fn stop_agent(
        self: &Arc<Self>,
        reason: StopReason,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get_mut(&tid) {
                session.pending_stop = Some(reason);
                session.stop.notify_one();
                Ok(())
            } else {
                drop(guard);
                if self.cancel_thread(tid, actor.clone()) {
                    return Ok(());
                }
                let discard = matches!(reason, StopReason::Discard);
                let stale_result = self
                    .end_stale_waiting_session(tid, discard, actor.clone())
                    .await;
                let settled = settle_stuck_running_thread(
                    &self.pool,
                    &self.event_bus,
                    tid,
                    actor,
                    SettleTerminal::StuckProjection,
                )
                .await?;
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
                session.pending_stop = Some(reason);
                session.stop.notify_one();
            }
            Ok(())
        }
    }

    /// Snapshot a session's `pending_stop` reason. Encapsulates the lock
    /// acquisition so the run_session loop and its post-loop cleanup don't
    /// have to know which field carries the stop reason — flipping `bool`
    /// fields back and forth here used to be the source of the phantom
    /// `ResponseCanceled` regressions.
    pub(crate) async fn pending_stop_reason(&self, thread_id: Uuid) -> Option<StopReason> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .and_then(|s| s.pending_stop)
    }

    /// Read and clear the device that clicked Cancel on a live session (stamped
    /// by `interrupt_agent`). The run_session interrupt arm calls this so the
    /// emitted `ResponseCanceled.actor` records the cancelling device. Drained
    /// on read — a resumed session must not carry a stale actor into its next
    /// turn. The `agent_sessions` analog of `take_cancel_actor` (the chat
    /// path's `ThreadHandle.cancel_actor` slot in `active_threads`).
    pub(crate) async fn take_session_cancel_actor(&self, thread_id: Uuid) -> Option<MessageOrigin> {
        self.agent_sessions
            .lock()
            .await
            .get_mut(&thread_id)
            .and_then(|s| s.cancel_actor.take())
    }

    /// Read and clear the redirect flag set by `arm_followup_redirect` when a
    /// follow-up interrupts a mid-turn coding-agent turn. The run_session interrupt arm
    /// calls this so the interrupted turn's `ResponseCanceled` carries
    /// `CancelCause::SupersededByFollowup` (neutral render) rather than
    /// `UserStop`. Drained on read — a resumed session must not carry a stale
    /// redirect flag into its next turn. The redirect analog of
    /// `take_session_cancel_actor`.
    pub(crate) async fn take_session_redirect_followup(&self, thread_id: Uuid) -> bool {
        self.agent_sessions
            .lock()
            .await
            .get_mut(&thread_id)
            .map(|s| std::mem::take(&mut s.redirect_followup))
            .unwrap_or(false)
    }

    /// Interrupt a running Claude Code session — sends control_request:interrupt to stop
    /// current work without killing the session (like pressing Esc in the CC terminal).
    /// The CC process stays alive and enters waiting state.
    ///
    /// `actor` flows onto the `ResponseCanceled` emitted by the no-session
    /// settle fallback so the panel reads "You" instead of "System".
    ///
    /// Fallbacks when no live Claude Code subprocess exists for `thread_id`:
    ///   1. Cancel the in-process agentic loop via the `active_threads` token
    ///      (handles run_thread-spawned chat threads and CC threads that are
    ///      mid-startup, before SessionStarted has registered an agent_session).
    ///      When the cancel lands, `run_session`'s `chat_cancel` arm emits the
    ///      terminal `ResponseCanceled` — skip the settle to avoid double-emit.
    ///   2. Only if the cancel had no entry to land on, settle so the UI
    ///      unsticks. `terminal` picks the event: `CanceledQuestion` when this
    ///      request cancel-stamped a pending question (the caller's own write
    ///      is what put the row at `running`), `StuckProjection` otherwise.
    ///      See [`SettleTerminal`].
    ///
    /// Unlike `stop_agent`, interrupt is always a real Cancel/Stop click — the
    /// frontend offers no "interrupt for Apply/Discard/Archive" path.
    ///
    /// Returns `Ok(true)` when a live turn was interrupted, an in-process loop
    /// canceled, or a stuck `running` projection settled — i.e. a terminal event
    /// is on its way. `Ok(false)` when nothing was interruptible (the settle
    /// found no stuck projection, or — for the thread-id-less variant — every
    /// session was already waiting). The handler folds this into the
    /// `{"canceled": bool}` response so the client can self-heal a stale view.
    /// The `SESSION_ALREADY_WAITING` `Err` is preserved for the single-thread
    /// idle-session race so the stop handler keeps its no-op-200 mapping.
    pub async fn interrupt_agent(
        &self,
        thread_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
        terminal: SettleTerminal,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut guard = self.agent_sessions.lock().await;

        if let Some(tid) = thread_id {
            if let Some(session) = guard.get_mut(&tid) {
                if session.is_waiting {
                    return Err(SESSION_ALREADY_WAITING.into());
                }
                // Record who clicked Cancel BEFORE the notify so the run_session
                // interrupt arm can drain it and stamp `ResponseCanceled.actor`
                // with the originating device (the popover's Device row).
                session.cancel_actor = actor;
                session.interrupt.notify_one();
                Ok(true)
            } else {
                drop(guard);
                if self.cancel_thread(tid, actor.clone()) {
                    return Ok(true);
                }
                settle_stuck_running_thread(&self.pool, &self.event_bus, tid, actor, terminal).await
            }
        } else {
            if guard.is_empty() {
                return Err("No Claude Code process is running".into());
            }
            let mut any = false;
            for session in guard.values() {
                if !session.is_waiting {
                    session.interrupt.notify_one();
                    any = true;
                }
            }
            Ok(any)
        }
    }

    /// Send a control request to a running Claude Code session via its control channel.
    pub async fn send_agent_control_request(
        &self,
        thread_id: Uuid,
        request: crate::runtime::ControlRequest,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event = crate::engine::thread_events::ThreadEvent::from_control_request(
            &request,
            self.thread_coding_agent(thread_id).await,
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
                        channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                        actor,
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
        let rows: Vec<(serde_json::Value,)> = match sqlx::query_as(
            "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
             ORDER BY created DESC LIMIT 10",
        )
        .bind(&tid)
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log!(
                    "[ClaudeCode] cc_thread_settings({}) DB error: {} — falling back to no per-thread overrides",
                    thread_id,
                    e
                );
                Vec::new()
            }
        };
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
            let has_active = s.is_live();
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
        let info = if let Some(repo_key) = repo_root {
            let cache = self.cc_commands_cache.read().await;
            lookup_repo_commands_in_cache(&cache, &repo_key)
        } else {
            // Thread has no recorded repo (no `changes` row yet) — return
            // empty rather than leaking another repo's skills.
            CcCommandsInfo::default()
        };
        let (model, effort) = self.cc_thread_settings(thread_id).await;
        CcCommandsResult {
            info,
            has_active_session: false,
            current_model: model,
            current_reasoning_effort: effort,
        }
    }

    /// Return cached commands for a specific repo path (for compose-view menu).
    /// Empty result if the repo has never had a Claude Code session — never falls back
    /// to another repo's cache.
    pub async fn cc_commands_for_repo(&self, repo_path: &Path) -> CcCommandsResult {
        let cache = self.cc_commands_cache.read().await;
        let info = lookup_repo_commands_in_cache(&cache, &repo_path.to_string_lossy());
        CcCommandsResult {
            info,
            has_active_session: false,
            current_model: None,
            current_reasoning_effort: None,
        }
    }
}
