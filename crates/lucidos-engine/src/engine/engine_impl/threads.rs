use super::super::*;
use crate::engine::PreEmittedOrigin;

const STUCK_TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub struct NamingSlot {
    being_named: Arc<std::sync::Mutex<std::collections::HashSet<uuid::Uuid>>>,
    thread_id: uuid::Uuid,
}

impl Drop for NamingSlot {
    fn drop(&mut self) {
        if let Ok(mut naming) = self.being_named.lock() {
            naming.remove(&self.thread_id);
        }
    }
}

impl LucidosEngine {
    pub fn embedder(&self) -> &Arc<crate::memory::EmbedderSlot> {
        &self.embedder
    }

    pub fn memory_index(&self) -> &Option<PgVectorIndex> {
        &self.memory_index
    }

    pub fn is_rebuilding_memory(&self) -> bool {
        self.rebuilding_memory.load(Ordering::SeqCst)
    }

    pub fn cancel_memory_rebuild(&self) {
        self.cancel_rebuild.store(true, Ordering::SeqCst);
    }

    /// INVARIANT: Tell this thread's handle that the agentic loop just took `n` injected
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

    pub fn inject_into_live_turn(&self, thread_id: Uuid, prompt: InjectedPrompt) -> bool {
        let threads = self.active_threads.lock().unwrap();
        match threads.get(&thread_id) {
            Some(handle) => handle.inject(prompt),
            None => false,
        }
    }

    pub fn hand_over_or_admit(
        &self,
        thread_id: Uuid,
        prompts: Vec<InjectedPrompt>,
    ) -> OrphanAdmission {
        crate::engine::hand_over_or_admit(
            &self.active_threads,
            &self.thread_completion,
            thread_id,
            prompts,
        )
    }

    pub fn inject_the_rest_of_a_sentence(
        &self,
        thread_id: Uuid,
        asked_turn: Uuid,
        prompt: InjectedPrompt,
    ) -> crate::engine::RestOfSentence {
        crate::engine::inject_the_rest_of_a_sentence(
            &self.active_threads,
            thread_id,
            asked_turn,
            prompt,
        )
    }

    pub fn note_history_read_begun(&self, thread_id: Uuid, generation: u64) {
        crate::engine::record_history_read_begun(&self.active_threads, thread_id, generation);
    }

    pub fn try_register_thread(&self, thread_id: Uuid) -> Option<ThreadRegistration> {
        crate::engine::try_register_thread(&self.active_threads, &self.thread_completion, thread_id)
            .admitted()
    }

    pub async fn register_thread_queued(&self, thread_id: Uuid) -> ThreadRegistration {
        crate::engine::admit_with_stuck_turn_eviction(
            &self.active_threads,
            &self.thread_completion,
            thread_id,
            STUCK_TURN_TIMEOUT,
            || async {
                // INVARIANT: Engine-initiated abort: emit ResponseAborted with actor=System
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

    /// INVARIANT: Remove a chat thread's `ThreadHandle` from `active_threads`, cancel its
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

    /// INVARIANT: Cancel a live Lucidos Agent turn because an urgent child follow-up is
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
        self.cancel_thread_with_cause(
            thread_id,
            actor,
            crate::engine::thread_events::CancelCause::SupersededByFollowup,
        )
    }

    pub fn cancel_thread_for_hold(
        &self,
        thread_id: Uuid,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> bool {
        self.cancel_thread_with_cause(
            thread_id,
            actor,
            crate::engine::thread_events::CancelCause::Hold,
        )
    }

    fn cancel_thread_with_cause(
        &self,
        thread_id: Uuid,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
        cause: crate::engine::thread_events::CancelCause,
    ) -> bool {
        if let Some(handle) = self.active_threads.lock().unwrap().get(&thread_id) {
            if actor.is_some() {
                *handle.cancel_actor.lock().unwrap() = actor;
            }
            *handle.cancel_cause.lock().unwrap() = Some(cause);
            handle.token.cancel();
            true
        } else {
            false
        }
    }

    pub fn take_cancel_cause(
        &self,
        thread_id: Uuid,
    ) -> Option<crate::engine::thread_events::CancelCause> {
        self.active_threads
            .lock()
            .unwrap()
            .get(&thread_id)
            .and_then(|h| h.cancel_cause.lock().unwrap().take())
    }

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

    pub async fn notify_parent_of_child_completion(
        self: &Arc<Self>,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
        child_terminal_event_id: Option<Uuid>,
        parent_is_coding_agent: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Same formatter `build_messages_now` uses on reload, so the
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
        if let Some(boundary) = crate::core::snapshot::boundary::read(self.pool()).await? {
            if row
                .sequence
                .is_none_or(|seq| seq <= boundary.event_sequence)
            {
                return Ok(());
            }
        }
        if !crate::core::snapshot::boundary::thread_turn_after_restore(self.pool(), child_thread_id)
            .await?
        {
            return Ok(());
        }
        // INVARIANT: **The parent may have been waiting for this itself.** A thread can
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
            crate::log_legacy!(
                "[FanOut] Parent {} was awaiting this completion; its event wait carries the \
                 wake, so the fan-in callback stands down (one turn, not two)",
                parent_thread_id
            );
            return Ok(());
        }

        let statuses = self
            .event_store
            .reported_change_statuses(std::slice::from_ref(&row))
            .await;
        let block = crate::core::store::format_child_thread_completed_block(&row, &statuses);

        let callback_origin = Some(thread_events::MessageOrigin::thread_link_child(
            child_thread_id,
            thread_events::ActorMode::Agent,
        ));
        self.agent_host()
            .process_message_with_steps(
                self,
                &block,
                None,
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
                None,
                thread_events::ActorMode::Agent,
                None,
                None,
                Some(PreEmittedOrigin::EngineReentry(child_completed_event_id)),
                None,
                callback_origin,
                None,
                crate::engine::FollowUpUrgency::Normal,
                None,
                None,
            )
            .await
            .map(|_| ())
    }

    /// INVARIANT: Will one of the parent's own *event waits* carry this
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
        if !self.live_waits.is_empty().await {
            let live = self.live_waits.for_thread(parent_thread_id).await;
            if !crate::engine::event_wait::waits_matching_row(&live, row).is_empty() {
                return true;
            }
        }

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

    pub fn processing_thread_ids(&self) -> Vec<Uuid> {
        self.active_threads
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect()
    }

    pub(crate) fn drain_orphaned_injections(
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        let mut orphans = Vec::new();
        while let Ok(prompt) = injection_rx.try_recv() {
            orphans.push(prompt);
        }
        orphans
    }

    /// INVARIANT: Tear down a finished chat turn and recover any follow-up that raced the
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
    pub fn finalize_turn_and_drain_injections(
        guard: ThreadGuard,
        injection_rx: &mut mpsc::UnboundedReceiver<InjectedPrompt>,
    ) -> Vec<InjectedPrompt> {
        drop(guard);
        Self::drain_orphaned_injections(injection_rx)
    }

    /// INVARIANT: Take this thread's naming slot, or answer `None` because a name is
    /// already on its way.
    ///
    /// The window the namer's "no title yet" read cannot close on its own:
    /// naming takes a model call, so two askers a second apart both read "no
    /// name yet". The slot is held through the namer's retry waits too.
    ///
    /// A poisoned lock takes the slot. A namer panicked mid-flight, and naming
    /// again risks the second title this guard prevents. Refusing would leave
    /// the thread unnameable for the life of the process.
    pub fn claim_the_naming_of(&self, thread_id: uuid::Uuid) -> Option<NamingSlot> {
        let taken = match self.threads_being_named.lock() {
            Ok(mut naming) => naming.insert(thread_id),
            Err(_) => true,
        };
        taken.then(|| NamingSlot {
            being_named: Arc::clone(&self.threads_being_named),
            thread_id,
        })
    }

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
