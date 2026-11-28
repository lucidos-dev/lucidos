use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use super::{
    catch_up_from_watermark, emit_cancel, emit_delivery, emit_expiry, expired_waits,
    is_awaitable_event, rebuild_live_waits, CancelWaitOutcome, LiveWait, WaitReentry,
    WaitReentryRequest, DEADLINE_SWEEP_INTERVAL,
};
use crate::core::event_subscription::{
    is_subscribable_system_event, matchable_system_payload, matchable_thread_payload,
    validate_subscribable_event_type, SubscriptionSurface, SubscriptionVerdict,
};
use crate::engine::event_bus::{BusEvent, EmittedEvent, LagReplay, ReplayScope, SystemEvent};
use crate::engine::thread_events::{ActorMode, EventWaitCancelCause, ThreadEvent};
use crate::engine::{LucidosEngine, PreEmittedOrigin};

const WAIT_ENDING_EVENT_TYPES: [&str; 2] = ["ThreadArchived", "ThreadDiscarded"];

#[derive(Debug)]
struct DispatcherStep {
    ended: Vec<(Uuid, EventWaitCancelCause)>,
    offer: Option<EmittedEvent>,
    rescan: bool,
}

async fn dispatcher_step(
    received: Result<EmittedEvent, RecvError>,
    replay: &mut LagReplay,
    pool: &sqlx::PgPool,
) -> Option<DispatcherStep> {
    match received {
        Ok(emitted) => Some(DispatcherStep {
            ended: replay
                .live(&emitted)
                .then(|| wait_ending(&emitted))
                .flatten()
                .into_iter()
                .collect(),
            offer: Some(emitted),
            rescan: false,
        }),
        Err(RecvError::Lagged(n)) => {
            // INVARIANT: The ends come first, so no wait on an archived thread is left
            // for the rescan to deliver. The rescan then covers the matches:
            // each wait scans from its own watermark, which never moves.
            crate::log_legacy!(
                "[EventWait] EventBus subscriber lagged by {} events, replaying archives \
                 and re-scanning every live wait",
                n
            );
            Some(DispatcherStep {
                ended: replay
                    .missed(pool)
                    .await
                    .iter()
                    .filter_map(wait_ending)
                    .collect(),
                offer: None,
                rescan: true,
            })
        }
        Err(RecvError::Closed) => None,
    }
}

fn wait_ending(emitted: &EmittedEvent) -> Option<(Uuid, EventWaitCancelCause)> {
    let BusEvent::Thread {
        thread_id, event, ..
    } = &emitted.typed
    else {
        return None;
    };
    let cause = match event {
        ThreadEvent::ThreadArchived => EventWaitCancelCause::ThreadArchived,
        ThreadEvent::ThreadDiscarded { .. } => EventWaitCancelCause::ThreadDiscarded,
        _ => return None,
    };
    Some((*thread_id, cause))
}

/// INVARIANT: The least pause between two sweeps, so a slow one never runs back to back.
const MIN_SWEEP_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// INVARIANT: How long to pause after a sweep that took `elapsed`. Sweeps start every
/// `DEADLINE_SWEEP_INTERVAL`, which the keep-alive beat windows rely on, and
/// a slow sweep stretches the period by `MIN_SWEEP_GAP` at most.
fn sweep_pause(elapsed: std::time::Duration) -> std::time::Duration {
    DEADLINE_SWEEP_INTERVAL
        .saturating_sub(elapsed)
        .max(MIN_SWEEP_GAP)
}

impl LucidosEngine {
    /// INVARIANT: Start the dispatcher's two background tasks: the bus subscriber and the
    /// deadline sweep.
    ///
    /// **Start this BEFORE [`Self::rebuild_event_waits`].** An event landing in
    /// between is seen by a subscriber whose cache is still empty and is
    /// dropped, and then picked up by the rebuild's catch-up scan, because
    /// every wait's watermark precedes it. The other order leaves a genuine
    /// hole: the rebuild finishes, an event lands, and nobody is listening yet.
    pub fn start_event_wait_dispatcher(self: &Arc<Self>) {
        let (mut rx, mut replay) = LagReplay::subscribe(
            &self.event_bus,
            ReplayScope::Workspace,
            Some(WAIT_ENDING_EVENT_TYPES.to_vec()),
        );
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(step) = dispatcher_step(rx.recv().await, &mut replay, &engine.pool).await
            {
                for (thread_id, cause) in step.ended {
                    engine.cancel_waits_ended_by(thread_id, cause).await;
                }
                if let Some(emitted) = step.offer {
                    engine.offer_event_to_waits(&emitted).await;
                }
                if step.rescan {
                    engine.catch_up_all_event_waits().await;
                }
            }
            crate::log_legacy!("[EventWait] EventBus closed, stopping wait dispatcher");
        });

        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                if engine.is_shutting_down() {
                    break;
                }
                let started = tokio::time::Instant::now();
                engine.sweep_expired_event_waits().await;
                engine.beat_cache_keep_alives().await;
                tokio::time::sleep(sweep_pause(started.elapsed())).await;
            }
        });
    }

    /// INVARIANT: True when the engine is tearing down, so this entry point must leave
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
        crate::log_legacy!(
            "[EventWait] Engine shutting down, not resolving waits from the {} path. \
             They stay armed, and the next engine's catch-up scan delivers them",
            site
        );
        true
    }

    async fn cancel_waits_ended_by(&self, thread_id: Uuid, cause: EventWaitCancelCause) {
        if self.live_waits.is_empty().await {
            return;
        }
        self.cancel_event_waits_for_thread(thread_id, cause, None)
            .await;
    }

    async fn offer_event_to_waits(self: &Arc<Self>, emitted: &EmittedEvent) {
        if self.live_waits.is_empty().await {
            return;
        }
        if self.shutdown_declines_reentry("live match") {
            return;
        }
        let (event_type, payload) = match &emitted.typed {
            BusEvent::Thread {
                thread_id,
                event,
                meta,
            } => {
                if !is_awaitable_event(event) {
                    return;
                }
                (
                    event.event_type().to_string(),
                    matchable_thread_payload(event, meta, *thread_id),
                )
            }
            BusEvent::System(se)
                if se.stored_event_type() == SystemEvent::SUB_THREAD_CARD_OPENED
                    && !matches!(se, SystemEvent::SubThreadCardOpened { .. }) =>
            {
                return
            }
            BusEvent::System(se) if is_subscribable_system_event(se) => (
                se.stored_event_type().to_string(),
                matchable_system_payload(se),
            ),
            _ => return,
        };

        let snapshot = self.live_waits.snapshot().await;
        for wait in snapshot {
            let Some(matched_index) = wait.matched_index_at(&event_type, &payload, emitted.seq)
            else {
                continue;
            };
            let (event_id, event_type, payload, matched_index) =
                if event_type == SystemEvent::SUB_THREAD_CARD_OPENED {
                    // Look up the earliest visible hit before claiming the
                    // wait, so the next live frame cannot overtake this one.
                    let Some(hit) = self.scan_event_wait(&wait).await else {
                        continue;
                    };
                    hit
                } else {
                    (
                        emitted.event_id,
                        event_type.clone(),
                        payload.clone(),
                        matched_index,
                    )
                };
            if self.shutdown_declines_reentry("live match") {
                return;
            }
            let Some(wait) = self.live_waits.take(wait.wait_id).await else {
                continue;
            };
            // INVARIANT: Detached so the bus subscriber never blocks on a chat turn. The
            // wait is already out of the cache, so nothing can double-resolve
            // it while this runs.
            let engine = self.clone();
            tokio::spawn(async move {
                engine
                    .deliver_event_wait(&wait, event_id, &event_type, &payload, matched_index)
                    .await;
            });
        }
    }

    pub(crate) async fn deliver_event_wait(
        &self,
        wait: &LiveWait,
        event_id: Uuid,
        event_type: &str,
        payload: &Value,
        matched_index: usize,
    ) {
        crate::log_legacy!(
            "[EventWait] Delivering {} to thread {} (wait {}, entry {})",
            event_type,
            wait.thread_id,
            wait.wait_id,
            matched_index,
        );
        self.note_delivered_completion(wait.thread_id, event_type, payload);
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

    /// INVARIANT: Resolve one wait as expired and re-enter its thread.
    ///
    /// An expiry **re-enters**: a silently dropped wait is a permanently stalled
    /// thread, which is strictly worse than the polling this replaces.
    pub(crate) async fn expire_event_wait(&self, wait: &LiveWait) {
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
        crate::log_legacy!(
            "[EventWait] Wait {} on thread {} timed out",
            wait.wait_id,
            wait.thread_id,
        );
        match emit_expiry(&self.event_bus, wait, &never_seen).await {
            Ok(reentry) => self.queue_wait_reentry(wait.thread_id, reentry),
            Err(e) => self.resolution_emit_failed(wait, "Expiry", e).await,
        }
    }

    /// INVARIANT: A resolution emit failed, so nothing was written and the wait is still
    /// live. Put it back.
    ///
    /// `take` has already removed it from the cache, so doing nothing here is
    /// not neutral: it strands the thread. No event could match the wait and
    /// no sweep could expire it, while its `EventWaitStarted` stays unresolved
    /// in the store until a restart notices.
    async fn resolution_emit_failed(
        &self,
        wait: &LiveWait,
        what: &str,
        error: Box<dyn std::error::Error + Send + Sync>,
    ) {
        crate::log_legacy!(
            "[EventWait] {} emit failed for wait {} on thread {}: {}. Re-arming: \
             nothing was persisted, so the wait is still live",
            what,
            wait.wait_id,
            wait.thread_id,
            error
        );
        self.live_waits.insert(wait.clone()).await;
    }

    /// INVARIANT: Hand a resolved wait's re-entry to the consumer task.
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
            crate::log_legacy!(
                "[EventWait] Re-entry channel closed, thread {} will not resume: {}",
                thread_id,
                e
            );
        }
    }

    pub fn start_wait_reentry_consumer(self: &Arc<Self>) {
        let rx = crate::engine::WAIT_REENTRY_RX.with(|cell| cell.borrow_mut().take());
        let Some(mut rx) = rx else {
            crate::log_legacy!("[EventWait] Re-entry receiver missing, consumer not started");
            return;
        };
        let engine = self.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let engine = engine.clone();
                tokio::spawn(async move { engine.reenter_from_wait(req).await });
            }
            crate::log_legacy!("[EventWait] Re-entry channel closed, consumer exiting");
        });
    }

    async fn reenter_from_wait(&self, req: WaitReentryRequest) {
        let WaitReentryRequest { thread_id, reentry } = req;
        let is_coding_agent = match crate::engine::thread_agent(&self.pool, thread_id).await {
            Ok(agent) => agent == Some(crate::engine::ThreadAgent::CodingAgent),
            Err(e) => {
                crate::log_legacy!(
                    "[EventWait] Could not read which agent runs thread {thread_id}: {e}"
                );
                if let Err(e) = crate::engine::emit_routing_failure(
                    &self.event_bus,
                    thread_id,
                    "Lucidos could not tell which agent runs this thread, so a wait it was on did not wake it.",
                )
                .await
                {
                    crate::log_legacy!("[EventWait] Could not record the refused re-entry on thread {thread_id}: {e}");
                }
                return;
            }
        };
        if !crate::engine::thread_hold::may_start_turn(self.pool(), thread_id).await {
            crate::log_legacy!(
                "[EventWait] Thread {} holds its automatic wait re-entry for the owner",
                thread_id
            );
            return;
        }
        self.run_wait_reentry(thread_id, reentry, is_coding_agent)
            .await;
    }

    pub(crate) async fn run_wait_reentry(
        &self,
        thread_id: Uuid,
        reentry: WaitReentry,
        is_coding_agent: bool,
    ) {
        if let Err(e) = self
            .agent_host()
            .process_message_with_steps(
                self,
                &reentry.text,
                None,
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
                None,
                ActorMode::Agent,
                None,
                None,
                Some(PreEmittedOrigin::WaitReentry(reentry.anchor_event_id)),
                None,
                None,
                None,
                crate::engine::FollowUpUrgency::Normal,
                None,
                None,
            )
            .await
        {
            // INVARIANT: A log line and nothing else. An ATTACHED resolution has already
            // flipped the projection to `running`, so this turn MUST settle,
            // and it does: the setup paths that used to return `Err` ahead of
            // the loop's safety net now settle the exchange themselves. The
            // terminator this site used to add carried no anchor, so the
            // idempotency gate could not see it was a duplicate.
            crate::log_legacy!(
                "[EventWait] Re-entry turn failed for thread {}: {}",
                thread_id,
                e
            );
        }
    }

    /// INVARIANT: Resolve every wait whose deadline has passed. One tick of the sweep.
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
        let mut snapshot = self.live_waits.snapshot().await;
        if snapshot.is_empty() {
            return;
        }
        for wait in &mut snapshot {
            self.refresh_background_wait_timing(wait).await;
        }
        for wait_id in expired_waits(&snapshot, Utc::now()) {
            let Some(wait) = self.live_waits.take(wait_id).await else {
                continue;
            };
            let engine = self.clone();
            tokio::spawn(async move { engine.expire_event_wait(&wait).await });
        }
    }

    pub(crate) async fn catch_up_event_wait(&self, wait: &LiveWait) {
        if self.shutdown_declines_reentry("catch-up scan") {
            return;
        }
        let Some((event_id, event_type, payload, matched_index)) = self.scan_event_wait(wait).await
        else {
            return;
        };
        if self.shutdown_declines_reentry("catch-up scan") {
            return;
        }
        let Some(wait) = self.live_waits.take(wait.wait_id).await else {
            return;
        };
        self.deliver_event_wait(&wait, event_id, &event_type, &payload, matched_index)
            .await;
    }

    async fn scan_event_wait(
        &self,
        wait: &LiveWait,
    ) -> Option<(Uuid, String, serde_json::Value, usize)> {
        catch_up_from_watermark(&self.pool, wait)
            .await
            .inspect_err(|e| {
                crate::log_legacy!(
                    "[EventWait] Catch-up scan failed for wait {} on thread {}: {}",
                    wait.wait_id,
                    wait.thread_id,
                    e
                );
            })
            .ok()
            .flatten()
    }

    async fn catch_up_all_event_waits(&self) {
        for wait in self.live_waits.snapshot().await {
            self.catch_up_event_wait(&wait).await;
        }
    }

    /// INVARIANT: Rebuild the live-waits cache from the event store and run each wait's
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
                crate::log_legacy!(
                    "[EventWait] Live-wait rebuild failed: {}. Parked threads will not \
                     resume until the next restart",
                    e
                );
                return 0;
            }
        };
        if loaded > 0 {
            crate::log_legacy!(
                "[EventWait] Rebuilt {} live wait(s) from the event store",
                loaded
            );
        }
        self.catch_up_all_event_waits().await;
        loaded
    }

    /// INVARIANT: Re-drive re-entries lost to a restart: a resolution that was persisted but
    /// whose turn never ran (I3b).
    ///
    /// The rebuild above cannot help here, because the wait is resolved and
    /// must NOT be re-armed; what is missing is only the turn. Mirrors
    /// `refire_unprocessed_child_completions`, which recovers the identical
    /// shape for the child-completion fan-in.
    ///
    /// "Never ran" means the thread has no event after the resolution other
    /// than the resolution's own anchor, which is exactly one shape (see
    /// `emit_resolution`): the `PromptInjected`. Anything else after it (a
    /// `TextStreamed`, a `ToolCalled`, a terminator, a later `MessageReceived`)
    /// means the re-entry was consumed, so nothing is re-driven.
    pub async fn refire_unresolved_wait_reentries(&self) -> usize {
        let lost = match super::lost_wait_reentries(&self.pool).await {
            Ok(rows) => rows,
            Err(e) => {
                crate::log_legacy!(
                    "[EventWait] Lost-re-entry query failed: {}. Skipping the recovery \
                     sweep this boot",
                    e
                );
                return 0;
            }
        };
        for lost_reentry in &lost {
            crate::log_legacy!(
                "[EventWait] Re-driving a re-entry lost to restart on thread {}",
                lost_reentry.thread_id
            );
            self.queue_wait_reentry(lost_reentry.thread_id, lost_reentry.reentry.clone());
        }
        if !lost.is_empty() {
            crate::log_legacy!(
                "[EventWait] Re-drove {} event-wait re-entry(s) lost to engine restart",
                lost.len()
            );
        }
        lost.len()
    }

    pub async fn cancel_event_wait(
        &self,
        thread_id: Uuid,
        wait_id: Uuid,
        cause: EventWaitCancelCause,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> CancelWaitOutcome {
        // INVARIANT: Take-if-it-is-this-thread's, under the ONE lock. A membership check
        // followed by a separate take is two round-trips over a window in which
        // the wait can resolve, and it cannot tell the two misses apart.
        let Some(wait) = self.live_waits.take_on_thread(thread_id, wait_id).await else {
            return CancelWaitOutcome::NotLive;
        };
        if let Err(e) = emit_cancel(&self.event_bus, &wait, cause, actor).await {
            self.resolution_emit_failed(&wait, "Cancel", e).await;
            // INVARIANT: A failed cancel wrote nothing, and that is NOT `NotLive`: the
            // wait exists and the re-arm above put it back, so the button is
            // about to work again.
            return CancelWaitOutcome::EmitFailed;
        }
        CancelWaitOutcome::Canceled
    }

    /// INVARIANT: Cancel every live wait on a thread. Returns how many were canceled.
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

    pub async fn drop_waits_for_thread(&self, thread_id: Uuid) -> usize {
        let mut dropped = 0usize;
        for live in self.live_waits.for_thread(thread_id).await {
            if self.live_waits.take(live.wait_id).await.is_some() {
                dropped += 1;
            }
        }
        dropped
    }

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

    /// INVARIANT: The shared body of the two calls above: cancel the live waits on
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
            crate::log_legacy!(
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

#[cfg(test)]
#[path = "dispatcher_tests.rs"]
mod tests;
