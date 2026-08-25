//! Memory accessors and thread registration / cancellation / title generation.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;
use crate::engine::chat::PreEmittedOrigin;

/// How long a follow-up waits for the turn ahead of it before that turn is
/// treated as wedged and evicted. Named so the log line and the doc comment
/// cannot drift from the value.
const STUCK_TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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

    /// Tell this thread's handle that the agentic loop just took `n` injected
    /// prompts off the channel, so a blocking tool stops treating them as
    /// unread. No-op once the thread is deregistered.
    ///
    /// `generation` is the caller's own registration ([`ThreadGuard::generation`])
    /// and is load-bearing, for the same reason `ThreadGuard::drop` checks it:
    /// `register_thread_queued` force-evicts a turn stuck for 60 s and installs
    /// a NEW handle under the same thread_id while the old loop is still
    /// unwinding. A bare thread_id lookup would then let the old loop's drain
    /// decrement the new turn's counter — hiding a genuinely unread follow-up
    /// and putting `bash_output(wait_secs=…)` back to blocking through it. The
    /// old loop's prompts belong to a handle that no longer exists; dropping
    /// the decrement on the floor is exactly right.
    ///
    /// See [`ThreadHandle::pending_injections`].
    pub fn note_injections_drained(&self, thread_id: Uuid, generation: u64, n: usize) {
        if n == 0 {
            return;
        }
        if let Some(handle) = self
            .active_threads
            .lock()
            .unwrap()
            .get(&thread_id)
            .filter(|h| h.generation == generation)
        {
            handle.injections_drained(n);
        }
    }

    /// Record the `request_event_id` this turn stamps on its own events, so an
    /// abort emitted from outside the loop (restart teardown, stuck-turn
    /// eviction, shutdown sweep) terminates THIS turn rather than whichever
    /// originating-type event happens to be newest. Read back by
    /// `engine::in_flight_request_event_id`; see [`ThreadHandle::request_event_id`]
    /// for what goes wrong without it.
    ///
    /// `generation` is the caller's own registration ([`ThreadGuard::generation`])
    /// and is load-bearing; `record_request_event_id` documents why.
    pub fn set_thread_request_event_id(
        &self,
        thread_id: Uuid,
        generation: u64,
        request_event_id: Uuid,
    ) {
        crate::engine::record_request_event_id(
            &self.active_threads,
            thread_id,
            generation,
            request_event_id,
        );
    }

    /// The `injection_notify` + unread-count pair for a thread, for a tool that
    /// wants to stop blocking when the user says something. `None` once the
    /// thread is deregistered.
    pub fn injection_wakeup(
        &self,
        thread_id: Uuid,
    ) -> Option<(
        Arc<tokio::sync::Notify>,
        Arc<std::sync::atomic::AtomicUsize>,
    )> {
        self.active_threads
            .lock()
            .unwrap()
            .get(&thread_id)
            .map(|h| (h.injection_notify.clone(), h.pending_injections.clone()))
    }

    /// Admit one run on `thread_id`, or refuse because a turn already owns it.
    /// The engine's entry point to [`crate::engine::try_register_thread`], where
    /// the single-flight contract is documented.
    pub fn try_register_thread(&self, thread_id: Uuid) -> Option<ThreadRegistration> {
        crate::engine::try_register_thread(&self.active_threads, &self.thread_completion, thread_id)
            .admitted()
    }

    /// Register a thread, waiting for any existing request on the same thread
    /// to finish first. This queues follow-up messages instead of cancelling
    /// in-progress work. The user must explicitly cancel if they want to
    /// interrupt the current request.
    ///
    /// Safety: if the existing thread doesn't finish within
    /// [`STUCK_TURN_TIMEOUT`], it is force-cancelled and evicted. This prevents
    /// follow-up messages from hanging forever if a CC task gets stuck (e.g.,
    /// process crash without proper guard cleanup).
    ///
    /// Both exits take the slot through the single-flight functions, so this
    /// never installs a handle over a live turn. A caller that can coalesce a
    /// redundant wake should reach for [`Self::try_register_thread`] first and
    /// inject on a refusal. Queueing a second turn for one logical event is
    /// still a second paid model call.
    pub async fn register_thread_queued(&self, thread_id: Uuid) -> ThreadRegistration {
        crate::engine::admit_with_stuck_turn_eviction(
            &self.active_threads,
            &self.thread_completion,
            thread_id,
            STUCK_TURN_TIMEOUT,
            || async {
                // Engine-initiated abort: emit ResponseAborted with actor=System
                // BEFORE the eviction cancels the token. Without it the
                // downstream cancel arms default to ResponseCanceled, since
                // they read is_shutdown=false. The frontend renders that as a
                // user-initiated "Canceled", so users think they pressed Stop.
                emit_stuck_thread_eviction_abort(
                    &self.event_bus,
                    &self.pool,
                    &self.agent_sessions,
                    &self.active_threads,
                    thread_id,
                )
                .await;
            },
        )
        .await
    }

    /// Remove a chat thread's `ThreadHandle` from `active_threads`, cancel its
    /// token, and notify any completion waiters. The agentic loop's own
    /// `ThreadGuard::drop` will then no-op (generation mismatch).
    ///
    /// One caller: the `/api/v1/restart` chat pre-emit. Stripping the entry
    /// up-front takes the thread out of `processing_thread_ids()`. The
    /// `shutdown_active_threads` sweep then leaves it alone, instead of
    /// double-emitting a System abort over the device "Paused by restart"
    /// panel just persisted. The stuck-turn eviction does NOT use this:
    /// freeing the slot and taking it are one step there
    /// ([`crate::engine::evict_and_register`]).
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

    /// Cancel a live Lucidos Agent turn because an urgent child follow-up is
    /// superseding it. The Lucidos Agent's half of interrupt-and-redirect, and
    /// deliberately a distinct entry point from [`Self::cancel_thread`]: the
    /// two differ in what the interrupted turn is *called*, and the difference
    /// is user-visible. A Stop click is an abandonment (`UserStop`, rendered
    /// "Canceled x", reported to a parent as a terminal child outcome); a
    /// redirect is a steer (`SupersededByFollowup`, rendered neutrally, and
    /// excluded from the parent-callback terminal set so the parent is not
    /// woken with a false "child canceled" card for work that continues in the
    /// very next turn).
    ///
    /// The caller must NOT also inject the follow-up. The redirected message
    /// runs as the NEXT turn, routed through `register_thread_queued`, which
    /// waits for this turn to release the handle. Injecting first would race
    /// the loop's own drain: a prompt consumed by the dying turn's final
    /// iteration is neither answered nor left behind for the orphan chain.
    ///
    /// Returns `false` when the thread has no live turn, in which case there
    /// was nothing to preempt and the follow-up simply starts one.
    pub fn cancel_thread_for_followup(
        &self,
        thread_id: Uuid,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> bool {
        if let Some(handle) = self.active_threads.lock().unwrap().get(&thread_id) {
            if actor.is_some() {
                *handle.cancel_actor.lock().unwrap() = actor;
            }
            handle
                .redirect_followup
                .store(true, std::sync::atomic::Ordering::Release);
            handle.token.cancel();
            true
        } else {
            false
        }
    }

    /// Read and clear the redirect flag set by
    /// [`Self::cancel_thread_for_followup`]. The agentic-loop cancel arms call
    /// this to pick `SupersededByFollowup` over `UserStop`. Drained on read, so
    /// a stale flag cannot relabel the next turn on the same thread. The
    /// `active_threads` analog of `take_session_redirect_followup`.
    pub fn take_redirect_followup(&self, thread_id: Uuid) -> bool {
        self.active_threads
            .lock()
            .unwrap()
            .get(&thread_id)
            .is_some_and(|h| {
                h.redirect_followup
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
            })
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
    /// routing. The re-entry-vs-user distinction (suppressing duplicate
    /// exchange-starter events) lives on [`crate::engine::AgentInputKind::ReentryFromEngine`]
    /// and [`crate::engine::InjectedPromptKind::ReentryFromEngine`]; this caller
    /// triggers it by passing `pre_emitted_origin = Some(child_completed_event_id)`.
    pub async fn notify_parent_of_child_completion(
        self: &Arc<Self>,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
        child_terminal_event_id: Option<Uuid>,
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
        // **The parent may have been waiting for this itself.** A thread can
        // `await_event` on `ChildThreadCompleted`, and the card the fan-in just
        // emitted is a bus event like any other, so the event-wait dispatcher
        // matches it and drives its own wake. Both wakes then want the same
        // turn: on 2026-08-06 the fan-in's won the race, the wait's queued
        // behind it, and the 60 s stuck-turn backstop evicted the running turn
        // to let the second one in. That eviction is gone with the attached
        // shape, but two turns telling the parent one thing is still wrong.
        //
        // So the wait's wake wins and this one stands down. The card is already
        // persisted and the counter reconciled in its own transaction, so the
        // parent still learns everything it would have; only the duplicate turn
        // is dropped.
        if self
            .child_completion_has_an_event_wait(
                parent_thread_id,
                child_completed_event_id,
                child_terminal_event_id,
                &row,
            )
            .await
        {
            crate::log!(
                "[FanOut] Parent {} was awaiting this completion; its event wait carries the \
                 wake, so the fan-in callback stands down (one turn, not two)",
                parent_thread_id
            );
            return Ok(());
        }

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
            Some(PreEmittedOrigin::EngineReentry(child_completed_event_id)),
            None,
            callback_origin,
            crate::engine::FollowUpUrgency::Normal,
        )
        .await
        .map(|_| ())
    }

    /// Will one of the parent's own *event waits* carry this
    /// `ChildThreadCompleted` instead? See the call site for why the fan-in
    /// callback stands down when it will.
    ///
    /// **Two probes, because the two consumers race and either can win.** The
    /// fan-in and the event-wait dispatcher are both driven by the same
    /// post-commit broadcast, on separate tasks, in no fixed order:
    ///
    /// - The dispatcher has NOT run yet: the wait is still in the live cache and
    ///   still matches this event, so it is going to resolve it. Standing down
    ///   is safe, and the persisted-row probe alone would miss this (the row
    ///   does not exist yet), which is exactly the hole the first version of
    ///   this gate had. It read the row and nothing else, so it essentially
    ///   never fired.
    /// - The dispatcher HAS run: the wait is gone from the cache, but its
    ///   `EventWaitDelivered` names this event id.
    ///
    /// **The delivery may name the child's TERMINAL, not the card.** A wait
    /// armed on `CodingAgentIdled OR ChildThreadCompleted` for one child
    /// describes one logical event twice, and resolves on whichever arm lands
    /// first. So `child_terminal_event_id` is threaded here from the emit
    /// site. Matching on that id keeps a LATER completion of the same child
    /// wakeable, since its terminal is a different row.
    ///
    /// **Every probe answers "no" when it cannot run.** No is the recoverable
    /// direction: it costs a duplicate turn the user can read, where a wrong
    /// yes leaves the parent with a completion card and no reaction at all. The
    /// one gap this leaves is a wait taken from the cache whose delivery emit
    /// then fails and re-arms it, which is a transient-write path where a
    /// duplicate is again the right side to land on.
    async fn child_completion_has_an_event_wait(
        &self,
        parent_thread_id: Uuid,
        child_completed_event_id: Uuid,
        child_terminal_event_id: Option<Uuid>,
        row: &crate::core::EventRow,
    ) -> bool {
        // Probe 1, the live cache. Scoped to a wait that actually MATCHES this
        // event: a thread may hold several, and one watching something else
        // says nothing about this completion. Asked through `waits_matching_row`
        // so the question is put against the same *matchable payload* the
        // dispatcher will match, and not the stored column: a wait scoped with a
        // `thread_id` condition matches only the former, and the two answering
        // differently is a duplicate turn.
        if !self.live_waits.is_empty().await {
            let live = self.live_waits.for_thread(parent_thread_id).await;
            if !crate::engine::event_wait::waits_matching_row(&live, row).is_empty() {
                return true;
            }
        }

        // Probe 2, the persisted resolution. Either id counts: an OR-wait
        // resolves on one arm, and which arm won says nothing about whether
        // the parent has been told.
        let mut delivered_ids = vec![child_completed_event_id.to_string()];
        if let Some(terminal_id) = child_terminal_event_id {
            delivered_ids.push(terminal_id.to_string());
        }
        crate::engine::event_wait::wait_delivery_names_any(
            self.pool(),
            parent_thread_id,
            &delivered_ids,
        )
        .await
    }

    /// Get a reference to the memory extractor (for Flash title generation, etc.)
    pub fn extractor(&self) -> Option<&crate::memory::MemoryExtractor> {
        self.extractor.as_ref()
    }

    /// Get list of thread IDs with a live processing task (chat loop running).
    /// Does NOT include idle coding-agent sessions, which are tracked via
    /// `thread_summaries.status`.
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
            let call = match chat::title_call(&self.pool, extractor).await {
                Ok(call) => call,
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
                    &call,
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
