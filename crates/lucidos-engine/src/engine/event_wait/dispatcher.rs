//! The **re-entry half** of the event-wait dispatcher: the bus subscriber, the
//! deadline sweep, the boot rebuild, and the re-entry that actually re-opens a
//! subscribed thread.
//!
//! `super` holds the pure and queryable half (the matcher, the cache, the
//! catch-up scan, the result builders). Everything here needs an
//! `Arc<LucidosEngine>`, because a re-entry is a chat turn.
//!
//! # Three ways in, one way out
//!
//! A wait resolves from the live bus ([`LucidosEngine::offer_event_to_waits`]),
//! from the deadline sweep ([`LucidosEngine::sweep_expired_event_waits`]), or
//! from the catch-up scan the boot rebuild and the tool both run
//! ([`LucidosEngine::catch_up_event_wait`]). All three funnel through
//! [`super::LiveWaits::take`] first, so the one-shot guarantee (I7) is a
//! property of the cache rather than of each caller remembering to check.
//!
//! # Restart
//!
//! Five mechanisms, and the plan's I3 / I3b hang off them:
//!
//! 1. The wait is an event, so the record survives by construction.
//! 2. [`LucidosEngine::rebuild_event_waits`] re-derives the cache at boot.
//! 3. Each rebuilt wait re-runs its catch-up scan, so an event that landed
//!    while the engine was down still reaches its thread. The same scan closes
//!    the live race between `EventWaitStarted` and the cache insert.
//! 4. [`LucidosEngine::refire_unresolved_wait_reentries`] re-drives a resolution
//!    whose re-entry never ran, which is the one gap the first three leave: a crash
//!    after the resolution is persisted but before the turn re-entered.
//! 5. **A teardown declines to resolve at all.** Once
//!    `LucidosEngine::is_shutting_down` is true, all three resolution paths
//!    named at the top of this doc return before taking the wait out of the
//!    cache, so it stays live and unresolved and mechanisms 2 and 3 deliver it
//!    on the next engine. This is the deliberate reason a wait can survive a
//!    restart still armed with a match already in the store, or past its own
//!    deadline.
//!
//!    It is a restart mechanism because a re-entry is a chat TURN. Without the gate
//!    a match landing mid-teardown starts a fresh turn against an engine on its
//!    way out: on 2026-08-07 one ran for fourteen seconds and was thrown away,
//!    and because it became in-flight after the teardown pre-emit's snapshot it
//!    took the `shutdown_active_threads` fallback and settled `failed` with a
//!    manual Continue while its siblings settled `paused` and auto-resumed. The
//!    actor half of that is fixed separately (`LucidosEngine::teardown_actor`),
//!    and has to be: no flag read can close the window between the check and the
//!    turn registering. This half is what stops the wasted turn, and leaves the
//!    thread simply re-entered cleanly on the new engine instead.

use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use super::{
    catch_up_from_watermark, emit_cancel, emit_delivery, emit_expiry, expired_waits,
    is_awaitable_event, rebuild_live_waits, waits_matching, CancelWaitOutcome, LiveWait,
    ResolutionEmitError, WaitReentry, WaitReentryRequest, DEADLINE_SWEEP_INTERVAL,
};
use crate::core::event_subscription::{
    is_subscribable_system_event, matchable_system_payload, matchable_thread_payload,
    validate_subscribable_event_type, SubscriptionSurface, SubscriptionVerdict,
};
use crate::engine::event_bus::{BusEvent, EmittedEvent};
use crate::engine::thread_events::{ActorMode, EventMeta, EventWaitCancelCause};
use crate::engine::{LucidosEngine, PreEmittedOrigin};

impl LucidosEngine {
    /// Start the dispatcher's two background tasks: the bus subscriber and the
    /// deadline sweep.
    ///
    /// **Start this BEFORE [`Self::rebuild_event_waits`].** An event landing in
    /// between is seen by a subscriber whose cache is still empty and is
    /// dropped, and then picked up by the rebuild's catch-up scan, because
    /// every wait's watermark precedes it. The other order leaves a genuine
    /// hole: the rebuild finishes, an event lands, and nobody is listening yet.
    pub fn start_event_wait_dispatcher(self: &Arc<Self>) {
        let mut rx = self.event_bus.subscribe();
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(emitted) => {
                        engine.cancel_waits_ended_by(&emitted).await;
                        engine.offer_event_to_waits(&emitted).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // The skipped batch is exactly the gap the watermark
                        // scan was built for, so run it rather than logging the
                        // loss and moving on. Without this a burst of events
                        // (an e2e run, an Apply All) can drop the one match a
                        // thread was watching for, and the thread then sleeps to
                        // its deadline for no reason.
                        crate::log!(
                            "[EventWait] EventBus subscriber lagged by {} events, \
                             re-scanning every live wait",
                            n
                        );
                        engine.catch_up_all_event_waits().await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        crate::log!("[EventWait] EventBus closed, stopping wait dispatcher");
                        break;
                    }
                }
            }
        });

        let engine = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(DEADLINE_SWEEP_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if engine.is_shutting_down() {
                    break;
                }
                engine.sweep_expired_event_waits().await;
            }
        });
    }

    /// True when the engine is tearing down, so this entry point must leave
    /// every live wait exactly as it is.
    ///
    /// **Every one of the three resolution paths must consult this BEFORE
    /// `LiveWaits::take`, never after** (the live match, the catch-up scan, and
    /// the deadline sweep).
    /// After the take the wait is out of the cache, and a bare return then
    /// strands it in the worst state available: no event can match it, no sweep
    /// can expire it, and its `EventWaitStarted` is still unresolved in the
    /// store, so only a restart would ever notice. Before the take, declining
    /// costs nothing at all, because the wait stays live with its watermark
    /// intact and the next engine's rebuild plus catch-up scan finds the same
    /// match (mechanisms 2 and 3 in the module doc).
    ///
    /// Logged rather than silent: a re-entry held back is a thread that will not run
    /// until the next boot, which is the sort of thing that should be findable
    /// in the log rather than inferred from a gap in a transcript.
    fn shutdown_declines_reentry(&self, site: &'static str) -> bool {
        if !self.is_shutting_down() {
            return false;
        }
        crate::log!(
            "[EventWait] Engine shutting down, not resolving waits from the {} path. \
             They stay armed, and the next engine's catch-up scan delivers them",
            site
        );
        true
    }

    /// Cancel a thread's waits when the bus says the thread itself has ended.
    ///
    /// Archive and discard are handled here rather than at each endpoint for
    /// the reason the broadcast/subscribe rule exists: there is more than one
    /// way to archive a thread (the button, the cascade when a parent is
    /// archived, an agent tool) and this catches all of them at the one point
    /// they agree on, the persisted event. Leaving a wait live behind the
    /// archive curtain would re-open a thread the user considers closed.
    ///
    /// The other two causes are not derivable from an event and stay at their
    /// call sites, both of them explicit requests rather than consequences:
    /// **Stop waiting** targets one subscription, and an agent stand-down
    /// targets one or all of the calling thread's own.
    async fn cancel_waits_ended_by(&self, emitted: &EmittedEvent) {
        let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        else {
            return;
        };
        let cause = match event {
            crate::engine::thread_events::ThreadEvent::ThreadArchived => {
                EventWaitCancelCause::ThreadArchived
            }
            crate::engine::thread_events::ThreadEvent::ThreadDiscarded { .. } => {
                EventWaitCancelCause::ThreadDiscarded
            }
            _ => return,
        };
        // Cheap guard so the common case (any archive, on any thread, with no
        // waits anywhere) costs one map read rather than a `for_thread` clone.
        if self.live_waits.is_empty().await {
            return;
        }
        // Engine-internal: archive/discard arrive off the bus, and the actor
        // that caused them is already recorded on the ThreadArchived /
        // ThreadDiscarded event itself.
        self.cancel_event_waits_for_thread(*thread_id, cause, None)
            .await;
    }

    /// Offer one bus event to every live wait, resolving the ones it matches.
    ///
    /// Covers the same three carriers as the trigger matcher: non-streaming
    /// `BusEvent::Thread`, `SystemEvent::DomainEvent` (a workspace's own
    /// `emit_event`), and any persisted `SystemEvent`. Awaiting a
    /// `ReleasePublished` or a `BackupCompleted` is a first-class case, not an
    /// afterthought.
    ///
    /// Declines outright during a teardown (mechanism 5 in the module doc): the
    /// events an engine emits on its way down are exactly the ones a wait is
    /// most likely to match, and resolving one here starts a chat turn the
    /// teardown is about to kill.
    async fn offer_event_to_waits(self: &Arc<Self>, emitted: &EmittedEvent) {
        // Emptiness FIRST. Building the payload below is a full
        // `serde_json::to_value` of the event (a `ResponseGenerated` carries
        // the whole reply, a `ToolResult` a whole tool output), and the
        // overwhelmingly common case is zero live waits, so checking after
        // would allocate and discard a complete JSON tree on every event the
        // engine emits. Same one-map-read idiom as `cancel_waits_ended_by`.
        if self.live_waits.is_empty().await {
            return;
        }
        // AFTER the emptiness check, so a teardown on a workspace with nothing
        // armed says nothing, and one that really is holding a re-entry back says so
        // once per event it declined.
        if self.shutdown_declines_reentry("live match") {
            return;
        }
        let (event_type, payload) = match &emitted.typed {
            BusEvent::Thread {
                thread_id, event, ..
            } => {
                if !is_awaitable_event(event) {
                    return;
                }
                // The *matchable payload*, which is the same object the
                // trigger matcher builds (I8) and the same view the catch-up
                // scan reconstructs from the row. The meta a particular emit
                // stamped is deliberately not in it: the persisted column
                // carries it and this path cannot, so a `condition` on a meta
                // field would match one path and not the other. `thread_id` is
                // the one attribute both paths CAN supply authoritatively, the
                // carrier here and the `events.thread_id` column there, which
                // is why it is the one cross-cutting field a condition may
                // name.
                (
                    event.event_type().to_string(),
                    matchable_thread_payload(event, *thread_id),
                )
            }
            // The system-side gate, shared with the trigger fan-out so the two
            // offer the identical set (I8). It admits a workspace's own domain
            // event and any persisted frame (ADR 0113): a `BackupCompleted` has
            // no thread event and no domain event beside it, so this is the only
            // path to it.
            //
            // No thread id is injected. A system frame belongs to no thread and
            // its row's `thread_id` column is NULL, so supplying one here is
            // precisely the live-versus-replay split described above.
            // `matchable_system_payload` builds from `to_payload`, the function
            // the row itself is written from, for the same reason.
            BusEvent::System(se) if is_subscribable_system_event(se) => (
                se.stored_event_type().to_string(),
                matchable_system_payload(se),
            ),
            _ => return,
        };

        let snapshot = self.live_waits.snapshot().await;
        for (wait_id, matched_index) in waits_matching(&snapshot, &event_type, &payload) {
            // The one-shot gate: whoever wins `take` owns the resolution, so a
            // burst of matching events produces exactly one delivery per wait.
            let Some(wait) = self.live_waits.take(wait_id).await else {
                continue;
            };
            // Detached so the bus subscriber never blocks on a chat turn. The
            // wait is already out of the cache, so nothing can double-resolve
            // it while this runs.
            let engine = self.clone();
            let event_id = emitted.event_id;
            let event_type = event_type.clone();
            let payload = payload.clone();
            tokio::spawn(async move {
                engine
                    .deliver_event_wait(&wait, event_id, &event_type, &payload, matched_index)
                    .await;
            });
        }
    }

    /// Resolve one wait as delivered and re-enter its thread.
    ///
    /// The caller must already have taken the wait out of the live-waits cache.
    pub(crate) async fn deliver_event_wait(
        &self,
        wait: &LiveWait,
        event_id: Uuid,
        event_type: &str,
        payload: &Value,
        matched_index: usize,
    ) {
        crate::log!(
            "[EventWait] Delivering {} to thread {} (wait {}, entry {})",
            event_type,
            wait.thread_id,
            wait.wait_id,
            matched_index,
        );
        match emit_delivery(
            &self.event_bus,
            wait,
            event_id,
            event_type,
            payload,
            matched_index,
        )
        .await
        {
            Ok(reentry) => self.queue_wait_reentry(wait.thread_id, reentry),
            Err(e) => self.resolution_emit_failed(wait, "Delivery", e).await,
        }
    }

    /// Resolve one wait as expired and re-enter its thread.
    ///
    /// An expiry **re-enters**: a silently dropped wait is a permanently stalled
    /// thread, which is strictly worse than the polling this replaces.
    pub(crate) async fn expire_event_wait(&self, wait: &LiveWait) {
        // The one moment a never-emitted event name is worth mentioning:
        // registration confirms only that the subscription was accepted, and an
        // unknown name is accepted on purpose (it may be a domain event nobody
        // has emitted yet), so a typo stays invisible until its timeout.
        //
        // Only such a name can be a typo. A name the engine ships is real even
        // where this workspace has emitted none, so the note would be wrong
        // advice about a correct spelling.
        let mut never_seen = Vec::new();
        for sub in &wait.on {
            if !matches!(
                validate_subscribable_event_type(&sub.event_type, SubscriptionSurface::Wait),
                Ok(SubscriptionVerdict::UnknownName)
            ) {
                continue;
            }
            if !self.event_type_seen_before(&sub.event_type).await {
                never_seen.push(sub.event_type.clone());
            }
        }
        crate::log!(
            "[EventWait] Wait {} on thread {} timed out",
            wait.wait_id,
            wait.thread_id,
        );
        match emit_expiry(&self.event_bus, wait, &never_seen).await {
            Ok(reentry) => self.queue_wait_reentry(wait.thread_id, reentry),
            Err(e) => self.resolution_emit_failed(wait, "Expiry", e).await,
        }
    }

    /// A resolution emit failed. Decide, from WHICH emit failed, whether the
    /// wait is still the engine's to keep.
    ///
    /// `take` has already removed it from the cache, so doing nothing here is
    /// not neutral: it strands the thread. The wait would be gone from the live
    /// set, so no event can match it and no sweep can expire it, while its
    /// `EventWaitStarted` is still unresolved in the store, so only a restart
    /// would ever notice. That is the worst outcome available for a transient
    /// write failure, and strictly worse than the polling this replaces.
    async fn resolution_emit_failed(
        &self,
        wait: &LiveWait,
        what: &str,
        error: ResolutionEmitError,
    ) {
        match error {
            ResolutionEmitError::Unresolved(e) => {
                crate::log!(
                    "[EventWait] {} emit failed for wait {} on thread {}: {}. Re-arming: \
                     nothing was persisted, so the wait is still live",
                    what,
                    wait.wait_id,
                    wait.thread_id,
                    e
                );
                self.live_waits.insert(wait.clone()).await;
            }
            ResolutionEmitError::AnchorMissing(e) => {
                // NOT re-armed: the resolution IS persisted, so re-arming would
                // let this wait resolve a second time. The thread is left with
                // a resolution as its last word, which the ordinary restart
                // machinery settles (the orphan sweep closes the dangling call
                // and the user gets Continue).
                crate::log!(
                    "[EventWait] {} anchor emit failed for wait {} on thread {}: {}. \
                     The wait is resolved but the thread will not resume until it is \
                     continued or the engine restarts",
                    what,
                    wait.wait_id,
                    wait.thread_id,
                    e
                );
            }
        }
    }

    /// Hand a resolved wait's re-entry to the consumer task.
    ///
    /// The channel hop is required rather than tidy: registration runs its
    /// catch-up scan inline (S7), so an `await_event` call can resolve a wait in
    /// the same breath, and awaiting the turn from there would make
    /// `run_agentic_loop`'s future contain itself. See `WAIT_REENTRY_RX`.
    fn queue_wait_reentry(&self, thread_id: Uuid, reentry: WaitReentry) {
        if let Err(e) = self
            .wait_reentry_tx
            .send(WaitReentryRequest { thread_id, reentry })
        {
            crate::log!(
                "[EventWait] Re-entry channel closed, thread {} will not resume: {}",
                thread_id,
                e
            );
        }
    }

    /// Drain the re-entry channel, running one chat turn per resolved wait. Started
    /// once at boot, beside the other consumers.
    pub fn start_wait_reentry_consumer(self: &Arc<Self>) {
        let rx = crate::engine::WAIT_REENTRY_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            crate::log!("[EventWait] Re-entry receiver missing, consumer not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                // One task per re-entry: two threads re-entered by the same event must
                // not serialize behind each other, and admission control (the
                // Thread Queue, via `register_thread_queued`) is what bounds
                // the concurrency, not this loop.
                let engine = engine.clone();
                tokio::spawn(async move { engine.reenter_from_wait(req).await });
            }
            crate::log!("[EventWait] Re-entry channel closed, consumer exiting");
        });
    }

    /// Re-enter the thread on the back of a resolved wait.
    ///
    /// `PreEmittedOrigin::WaitReentry` is what keeps this from looking like
    /// something the user said: no `MessageReceived` is emitted, the turn's
    /// `request_event_id` is the anchor `emit_delivery` already wrote, and a
    /// thread that happens to be running injects it as
    /// `InjectedPromptKind::ReentryFromWait` rather than starting a second turn.
    ///
    /// **A coding-agent thread is re-entered through the coding-agent lane**, which
    /// is what makes `await_event` available to Claude Code and Codex at all.
    /// Passing `is_coding_agent` here picks the `msg_tx` route into a live
    /// session, or a fresh `--resume` when there is none, exactly as the
    /// child-completion fan-in does. An unreadable row answers `false`: re-entering a
    /// coding-agent thread down the chat lane wastes a turn and is recoverable,
    /// while re-entering a chat thread down the coding-agent lane would try to spawn
    /// a session for a thread that has no worktree.
    async fn reenter_from_wait(&self, req: WaitReentryRequest) {
        let WaitReentryRequest { thread_id, reentry } = req;
        let is_coding_agent: bool =
            sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or_else(|e| {
                    crate::log!(
                        "[EventWait] Could not read is_coding_agent for thread {}: {} \
                 (re-entering through the chat lane)",
                        thread_id,
                        e
                    );
                    None
                })
                .unwrap_or(false);
        if let Err(e) = self
            .process_message_with_steps(
                &reentry.text,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(is_coding_agent),
                None,
                Some(thread_id),
                None,
                None,
                None,
                None,
                None,
                ActorMode::Agent,
                None,
                None,
                Some(PreEmittedOrigin::WaitReentry(reentry.anchor_event_id)),
                None,
                None,
                crate::engine::FollowUpUrgency::Normal,
            )
            .await
        {
            // A log line and nothing else. An ATTACHED resolution has already
            // flipped the projection to `running`, so this turn MUST settle,
            // and it does: the setup paths that used to return `Err` ahead of
            // the loop's safety net now settle the exchange themselves. The
            // terminator this site used to add carried no anchor, so the
            // idempotency gate could not see it was a duplicate.
            crate::log!(
                "[EventWait] Re-entry turn failed for thread {}: {}",
                thread_id,
                e
            );
        }
    }

    /// Resolve every wait whose deadline has passed. One tick of the sweep.
    ///
    /// An expiry RE-ENTERS, so it is a chat turn like any other and needs the same
    /// teardown gate as the two match paths. The caller's loop already breaks on
    /// the flag, and that is not enough: it reads it once per tick, on a ten
    /// second interval, against a teardown that routinely takes longer than that
    /// (`shutdown_agent_sessions` alone polls for up to ten seconds). A tick that
    /// passes the loop check microseconds before `begin_teardown` still reaches
    /// the take below. Re-reading it here, immediately before the take, is what
    /// makes the contract in `shutdown_declines_reentry` true of every resolution
    /// path rather than of two out of three. The wait then stays armed past its
    /// deadline and `rebuild_event_waits` re-arms it on the next engine, whose
    /// own first sweep expires it (`rebuild_re_arms_a_wait_that_expired_while_the_engine_was_down`).
    async fn sweep_expired_event_waits(self: &Arc<Self>) {
        if self.shutdown_declines_reentry("deadline sweep") {
            return;
        }
        let snapshot = self.live_waits.snapshot().await;
        if snapshot.is_empty() {
            return;
        }
        for wait_id in expired_waits(&snapshot, Utc::now()) {
            let Some(wait) = self.live_waits.take(wait_id).await else {
                continue;
            };
            let engine = self.clone();
            tokio::spawn(async move { engine.expire_event_wait(&wait).await });
        }
    }

    /// Run one wait's catch-up scan and resolve it if anything already matched.
    ///
    /// Shared by registration and the boot rebuild, which is what makes the
    /// watermark close two gaps with one mechanism (S7): events that landed
    /// while the engine was down, and the live race between emitting
    /// `EventWaitStarted` and this module inserting the cache entry.
    ///
    /// Only the FIRST hit is used. A wait is a rendezvous, not a stream.
    ///
    /// Declines during a teardown for the same reason the live match path does.
    /// The registration caller is the one this reaches: an `await_event` armed
    /// while the engine is going down would otherwise scan, find its match, and
    /// re-enter the thread into a turn with seconds to live. The boot caller can
    /// never see the flag set, since a fresh engine is not shutting down.
    pub(crate) async fn catch_up_event_wait(&self, wait: &LiveWait) {
        if self.shutdown_declines_reentry("catch-up scan") {
            return;
        }
        let hit = match catch_up_from_watermark(&self.pool, wait).await {
            Ok(hit) => hit,
            Err(e) => {
                crate::log!(
                    "[EventWait] Catch-up scan failed for wait {} on thread {}: {}",
                    wait.wait_id,
                    wait.thread_id,
                    e
                );
                return;
            }
        };
        let Some((event_id, event_type, payload, matched_index)) = hit else {
            return;
        };
        let Some(wait) = self.live_waits.take(wait.wait_id).await else {
            // The live bus beat the scan to it. Correct, and the reason both
            // paths go through `take`.
            return;
        };
        self.deliver_event_wait(&wait, event_id, &event_type, &payload, matched_index)
            .await;
    }

    /// Run the catch-up scan for every live wait.
    ///
    /// Two callers, and they are the two ways an event can reach the store
    /// without reaching the matcher: the boot rebuild (the engine was down) and
    /// a lagged bus subscriber (the engine was up but behind). Idempotent, and
    /// safe to run at any time: the scan only looks forward from each wait's
    /// own watermark, and a wait that already resolved is not in the cache to
    /// be scanned.
    async fn catch_up_all_event_waits(&self) {
        for wait in self.live_waits.snapshot().await {
            self.catch_up_event_wait(&wait).await;
        }
    }

    /// Close every `await_event` call left unpaired by the pre-2026-08-06
    /// **attached** wait shape. Called once at boot, before the rebuild.
    ///
    /// An unpaired `tool_use` is a provider 400, so without this a thread that
    /// was mid-wait across the upgrade fails on its very next turn and there is
    /// no longer any code that would close the pair for it. Closing it is the
    /// whole of what is owed: a wait still unresolved is re-armed by
    /// `rebuild_event_waits` as an ordinary subscription and re-enters the thread
    /// the new way, and a wait already resolved left its payload in events the
    /// thread reads anyway.
    ///
    /// Temporary measure, registered in `docs/temporary-measures.md`: it is a
    /// no-op the moment no such thread is left.
    pub async fn settle_legacy_attached_event_waits(&self) -> usize {
        let stranded = match super::settle_legacy_attached_event_waits(&self.pool).await {
            Ok(rows) => rows,
            Err(e) => {
                crate::log!(
                    "[EventWait] Legacy attached-wait scan failed: {}. A thread caught \
                     mid-wait by the upgrade will fail its next turn on an unpaired \
                     tool_use until the next restart",
                    e
                );
                return 0;
            }
        };
        let mut closed = 0usize;
        for thread_id in &stranded {
            crate::log!(
                "[EventWait] Closing a legacy unpaired await_event call on thread {}",
                thread_id
            );
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id: *thread_id,
                        event: super::legacy_attached_settle_tool_result(),
                        meta: EventMeta::NONE,
                    },
                    "[EventWait] ToolResult (legacy attached wait settled)",
                )
                .await;
            closed += 1;
        }
        if closed > 0 {
            crate::log!(
                "[EventWait] Closed {} legacy unpaired await_event call(s)",
                closed
            );
        }
        closed
    }

    /// Rebuild the live-waits cache from the event store and run each wait's
    /// catch-up scan. Called once at boot.
    ///
    /// Returns how many waits were rebuilt. An expired one is rebuilt too
    /// rather than dropped: the deadline sweep resolves it on its next tick, so
    /// a wait whose deadline passed while the engine was down re-enters its thread
    /// with an expiry instead of vanishing (I3).
    pub async fn rebuild_event_waits(&self) -> usize {
        let loaded = match rebuild_live_waits(&self.pool, &self.live_waits).await {
            Ok(n) => n,
            Err(e) => {
                crate::log!(
                    "[EventWait] Live-wait rebuild failed: {}. Parked threads will not \
                     resume until the next restart",
                    e
                );
                return 0;
            }
        };
        if loaded > 0 {
            crate::log!(
                "[EventWait] Rebuilt {} live wait(s) from the event store",
                loaded
            );
        }
        self.catch_up_all_event_waits().await;
        loaded
    }

    /// Re-drive re-entries lost to a restart: a resolution that was persisted but
    /// whose turn never ran (I3b).
    ///
    /// The rebuild above cannot help here, because the wait is resolved and
    /// must NOT be re-armed; what is missing is only the turn. Mirrors
    /// `refire_unprocessed_child_completions`, which recovers the identical
    /// shape for the child-completion fan-in.
    ///
    /// "Never ran" means the thread has no event after the resolution other
    /// than the resolution's own anchor, which is exactly one shape (see
    /// `emit_resolution`): the `UserPromptInjected`. Anything else after it (a
    /// `TextStreamed`, a `ToolCalled`, a terminator, a later `MessageReceived`)
    /// means the re-entry was consumed, so nothing is re-driven.
    pub async fn refire_unresolved_wait_reentries(&self) -> usize {
        let lost = match super::lost_wait_reentries(&self.pool).await {
            Ok(rows) => rows,
            Err(e) => {
                crate::log!(
                    "[EventWait] Lost-re-entry query failed: {}. Skipping the recovery \
                     sweep this boot",
                    e
                );
                return 0;
            }
        };
        for lost_reentry in &lost {
            crate::log!(
                "[EventWait] Re-driving a re-entry lost to restart on thread {}",
                lost_reentry.thread_id
            );
            self.queue_wait_reentry(lost_reentry.thread_id, lost_reentry.reentry.clone());
        }
        if !lost.is_empty() {
            crate::log!(
                "[EventWait] Re-drove {} event-wait re-entry(s) lost to engine restart",
                lost.len()
            );
        }
        lost.len()
    }

    /// Cancel ONE wait by id: the **Stop waiting** button. Returns false when
    /// the wait is already gone, which is what the endpoint reports so a stale
    /// button does not claim to have done something.
    pub async fn cancel_event_wait(
        &self,
        thread_id: Uuid,
        wait_id: Uuid,
        cause: EventWaitCancelCause,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> CancelWaitOutcome {
        // Take-if-it-is-this-thread's, under the ONE lock. A membership check
        // followed by a separate take is two round-trips over a window in which
        // the wait can resolve, and it cannot tell the two misses apart.
        let Some(wait) = self.live_waits.take_on_thread(thread_id, wait_id).await else {
            return CancelWaitOutcome::NotLive;
        };
        if let Err(e) = emit_cancel(&self.event_bus, &wait, cause, actor).await {
            self.resolution_emit_failed(&wait, "Cancel", e).await;
            // A cancel writes one event and nothing else, so the only failure
            // it can report is `Unresolved`, and that is NOT `NotLive`: the
            // wait exists and the re-arm above put it back, so the button is
            // about to work again.
            return CancelWaitOutcome::EmitFailed;
        }
        CancelWaitOutcome::Canceled
    }

    /// Cancel every live wait on a thread. Returns how many were canceled.
    ///
    /// Two callers, and they are the two ways EVERY subscription on a thread
    /// ends at once: [`Self::cancel_waits_ended_by`] when the thread itself
    /// ends (archive or discard), and the agent standing all of its own down.
    /// A thread-level **Stop** is deliberately not one of them: it ends the
    /// turn and nothing else (see `api::chat::cancel_chat`). Neither is a user
    /// *message*: typing into a subscribed thread runs an ordinary turn and
    /// leaves every subscription exactly as it was, because none of them holds
    /// the turn to begin with.
    pub async fn cancel_event_waits_for_thread(
        &self,
        thread_id: Uuid,
        cause: EventWaitCancelCause,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> usize {
        self.cancel_waits_on_thread_where(thread_id, cause, actor, "all", |_| true)
            .await
    }

    /// Cancel every live wait on a thread that WATCHES `event_type`, leaving the
    /// rest alone. Returns how many were canceled.
    ///
    /// The narrow sibling of the call above, and the one an agent reaches for
    /// when it got its answer about one thing and is still waiting on another.
    /// Its caller is `cancel_event_waits_for_agent`, over
    /// `lucidos event-waits cancel --on` and the `cancel_event_wait` tool's
    /// `on`, and through those the e2e lock's own stand-down: taking the lock
    /// answers a watch for `E2ELockReleased` and nothing else, so it may end
    /// that watch and nothing else.
    pub async fn cancel_event_waits_watching(
        &self,
        thread_id: Uuid,
        event_type: &str,
        cause: EventWaitCancelCause,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> usize {
        self.cancel_waits_on_thread_where(thread_id, cause, actor, event_type, |w| {
            w.watches(event_type)
        })
        .await
    }

    /// The shared body of the two calls above: cancel the live waits on
    /// `thread_id` that `keep` selects, one emit each.
    ///
    /// `scope` names what was addressed, for the log line only. The predicate
    /// is applied to the snapshot rather than inside the cache's lock, and the
    /// `take` below is what makes that safe: a wait that resolved in between is
    /// simply not there any more, exactly as it is for an unfiltered cancel.
    async fn cancel_waits_on_thread_where(
        &self,
        thread_id: Uuid,
        cause: EventWaitCancelCause,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
        scope: &str,
        keep: impl Fn(&crate::engine::event_wait::LiveWait) -> bool,
    ) -> usize {
        let mut canceled = 0usize;
        for live in self.live_waits.for_thread(thread_id).await {
            if !keep(&live) {
                continue;
            }
            let Some(wait) = self.live_waits.take(live.wait_id).await else {
                continue;
            };
            if let Err(e) = emit_cancel(&self.event_bus, &wait, cause, actor.clone()).await {
                self.resolution_emit_failed(&wait, "Cancel", e).await;
                continue;
            }
            canceled += 1;
        }
        if canceled > 0 {
            crate::log!(
                "[EventWait] Canceled {} live wait(s) on thread {} ({}, {:?})",
                canceled,
                thread_id,
                scope,
                cause
            );
        }
        canceled
    }
}
