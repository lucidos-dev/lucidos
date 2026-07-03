//! Memory accessors and thread registration / cancellation / title generation.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;

impl LucidosEngine {
    /// Get a reference to the embedder for sharing with read-only handlers
    pub fn embedder(&self) -> &Arc<crate::memory::EmbedderSlot> {
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
                cancel_actor: Arc::new(std::sync::Mutex::new(None)),
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
            self.force_evict_chat_thread(thread_id);
        }

        self.register_thread(thread_id)
    }

    /// Remove a chat thread's `ThreadHandle` from `active_threads`, cancel its
    /// token, and notify any completion waiters. The agentic loop's own
    /// `ThreadGuard::drop` will then no-op (generation mismatch). Used by
    /// (a) `register_thread_queued`'s 60s force-eviction and (b) the
    /// `/api/v1/restart` chat pre-emit, where stripping the entry up-front
    /// removes the thread from `processing_thread_ids()` so the subsequent
    /// `shutdown_active_threads` sweep doesn't double-emit a System abort on
    /// top of the device "Restarted" panel we just persisted.
    pub(super) fn force_evict_chat_thread(&self, thread_id: Uuid) {
        if let Some(handle) = self.active_threads.lock().unwrap().remove(&thread_id) {
            handle.token.cancel();
        }
        if let Some(n) = self.thread_completion.lock().unwrap().remove(&thread_id) {
            n.notify_waiters();
        }
    }

    /// Cancel a specific thread. Returns `true` if the thread had an active
    /// `cancel_token` registered (the cancel landed and the per-thread loop
    /// will observe it). Returns `false` when there is no active entry — the
    /// caller can then fall back to settling the projection.
    ///
    /// `actor` records *who clicked Stop* so the agentic-loop cancel arm
    /// can stamp the emitted `ResponseCanceled` with the originating device.
    /// Pass `None` for engine-internal cancels (shutdown, restart) — those
    /// emit their own boundary events with explicit actor upstream, and the
    /// idempotency gate in `emit_response_canceled` suppresses the loop's
    /// follow-up emit.
    pub fn cancel_thread(
        &self,
        thread_id: Uuid,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> bool {
        if let Some(handle) = self.active_threads.lock().unwrap().get(&thread_id) {
            if actor.is_some() {
                *handle.cancel_actor.lock().unwrap() = actor;
            }
            handle.token.cancel();
            true
        } else {
            false
        }
    }

    /// Read and clear the pending cancel actor for `thread_id`. Returns
    /// `None` when no actor was stamped or the thread is not registered.
    /// The agentic-loop cancel arms call this when emitting
    /// `ResponseCanceled` so the event's `meta.actor` records the device
    /// that clicked Stop. Drained on read to avoid carrying a stale device
    /// across the next request that lands on the same thread.
    pub fn take_cancel_actor(
        &self,
        thread_id: Uuid,
    ) -> Option<crate::engine::thread_events::MessageOrigin> {
        self.active_threads
            .lock()
            .unwrap()
            .get(&thread_id)
            .and_then(|h| h.cancel_actor.lock().unwrap().take())
    }

    /// Unified entry point for FanOut: a child thread has completed and the
    /// parent needs to react. Routes through [`Self::process_message_with_steps`]
    /// — same fast-path / slow-path decisions as a user follow-up, no parallel
    /// routing. The wake-vs-user distinction (suppressing duplicate
    /// exchange-starter events) lives on [`crate::engine::AgentInputKind::WakeFromChild`]
    /// and [`crate::engine::InjectedPromptKind::WakeFromChild`]; this caller
    /// triggers it by passing `pre_emitted_origin = Some(child_completed_event_id)`.
    pub async fn notify_parent_of_child_completion(
        self: &Arc<Self>,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
        parent_is_coding_agent: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Same formatter `build_session_messages` uses on reload, so the
        // inflight wake matches what a post-restart resume would project.
        let row = match self
            .event_store
            .get_event_by_id(child_completed_event_id)
            .await?
        {
            Some(r) => r,
            None => {
                return Err(format!(
                    "ChildThreadCompleted event {} missing in DB",
                    child_completed_event_id
                )
                .into());
            }
        };
        let block = crate::core::store::format_child_thread_completed_block(&row);

        let callback_origin = Some(thread_events::MessageOrigin::thread_link_child(
            child_thread_id,
            thread_events::ActorMode::Agent,
        ));
        self.process_message_with_steps(
            &block,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(parent_is_coding_agent),
            None,
            Some(parent_thread_id),
            None,
            None,
            None,
            None,
            None,
            thread_events::ActorMode::Agent,
            None,
            None,
            Some(child_completed_event_id),
            None,
            callback_origin,
        )
        .await
        .map(|_| ())
    }

    /// Get a reference to the memory extractor (for Flash title generation, etc.)
    pub fn extractor(&self) -> Option<&crate::memory::MemoryExtractor> {
        self.extractor.as_ref()
    }

    /// Get list of thread IDs with a live processing task (chat loop running).
    /// Does NOT include idle coding-agent sessions — those are tracked via thread_summaries.status.
    pub fn processing_thread_ids(&self) -> Vec<Uuid> {
        self.active_threads
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect()
    }

    /// Drain any messages left on the injection channel after the agentic
    /// loop exited — see the call site in `chat/process.rs` for the race.
    pub(crate) fn drain_orphaned_injections(
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        let mut orphans = Vec::new();
        while let Ok(prompt) = injection_rx.try_recv() {
            orphans.push(prompt);
        }
        orphans
    }

    /// Tear down a finished chat turn and recover any follow-up that raced the
    /// teardown — closing the finalize-window race where a follow-up posted
    /// while the turn is going idle gets acknowledged but is NEITHER injected
    /// nor queued.
    ///
    /// **Ordering is the whole point.** This drops the [`ThreadGuard`] FIRST —
    /// removing the handle from `active_threads` (which drops the only
    /// `injection_tx`) and notifying completion waiters — and only THEN drains
    /// `injection_rx`. Removal-before-drain is what makes the recovery total:
    ///
    /// - The fast-path follow-up send takes the `active_threads` lock, looks up
    ///   the handle, and sends **while still holding that lock** (see
    ///   `chat/process/run.rs`). The guard's `Drop` removes the handle under the
    ///   SAME lock. So a racing send is serialized one of two ways:
    ///   - it completes fully *before* removal → the message is buffered on
    ///     `injection_rx` and this drain recovers it as an orphan (re-submitted
    ///     by the caller as a fresh follow-up = the next turn from the queue);
    ///   - it runs fully *after* removal → the lookup misses, the send is
    ///     reported failed, and the caller falls through to the slow path
    ///     (`register_thread_queued` starts a new turn). A failed inject is
    ///     harmless precisely because the message was never handed off.
    ///
    /// Draining *before* removal (the old order) left a window: a send landing
    /// after the drain but before the guard dropped was acknowledged into a
    /// channel nobody would ever read, then silently dropped on return.
    pub(crate) fn finalize_turn_and_drain_injections(
        guard: ThreadGuard,
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        // Remove from active_threads (drops injection_tx) + notify waiters,
        // closing the inject gate, THEN sweep anything already buffered.
        drop(guard);
        Self::drain_orphaned_injections(injection_rx)
    }

    /// Spawn background title generation for a thread (used when pinning).
    /// Looks up the first message of the thread and generates a title via Flash.
    pub async fn spawn_title_generation(&self, thread_id: &str) {
        let tid_uuid = match uuid::Uuid::parse_str(thread_id) {
            Ok(u) => u,
            Err(e) => {
                log!(
                    "[Title] spawn_title_generation skipped: invalid thread_id {:?}: {}",
                    thread_id,
                    e
                );
                return;
            }
        };
        if let Some(ref extractor) = self.extractor {
            let title_model = PreferenceStore::get(&self.pool, PREF_MODEL_TITLE)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let provider = match extractor.provider_for_model(&title_model) {
                Ok(p) => p,
                Err(e) => {
                    log!("[Thread] Failed to build title provider: {}", e);
                    return;
                }
            };
            let event_store = self.event_store.clone();
            let bus = self.event_bus.clone();
            let tid = thread_id.to_string();
            tokio::spawn(async move {
                let (first_msg, image_desc, image_count) =
                    match event_store.get_thread_first_message(&tid).await {
                        Ok(Some((msg, desc, count))) => (msg, desc, count),
                        Ok(None) => return,
                        Err(e) => {
                            log!("[Thread] Failed to get first message for title: {}", e);
                            return;
                        }
                    };

                chat::emit_generated_title(
                    &bus,
                    provider.as_ref(),
                    tid_uuid,
                    &first_msg,
                    image_desc.as_deref(),
                    None,
                    image_count,
                )
                .await;
            });
        }
    }

    /// Cancel all active threads. `actor` is stamped on every handle's
    /// cancel slot so the agentic-loop cancel arms can attribute the
    /// resulting `ResponseCanceled` events. Pass `None` for engine-internal
    /// cancels (shutdown, restart) — those already emit boundary events
    /// with explicit actor upstream. Returns whether any thread was active
    /// (so the thread-id-less `cancel_chat` can report an honest `canceled`).
    pub fn cancel_all_threads(
        &self,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> bool {
        let handles = self.active_threads.lock().unwrap();
        let any = !handles.is_empty();
        for handle in handles.values() {
            if let Some(ref a) = actor {
                *handle.cancel_actor.lock().unwrap() = Some(a.clone());
            }
            handle.token.cancel();
        }
        any
    }

}
