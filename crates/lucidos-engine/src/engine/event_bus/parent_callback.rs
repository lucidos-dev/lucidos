//! Child→parent fan-in.
//!
//! When a child thread reaches a terminal event, [`EventBus::emit`]'s PostCommit
//! phase drives these methods to decrement the parent's `active_children_count`,
//! surface the parent to inbox, and emit the typed `ChildThreadCompleted` onto
//! the parent thread. Extracted from `event_bus` verbatim — behavior-preserving.

use chrono::Utc;
use uuid::Uuid;

use super::{BusEvent, EmittedEvent, EventBus, ParentCallback};
use crate::engine::thread_events::{CancelCause, ChildCompletionStatus, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::ArchiveState;

/// DB row from thread_summaries for child-to-parent fan-out:
/// `(parent_thread_id, is_coding_agent, title, first_message,
/// parent_callback_pending, parent_is_coding_agent)`. The last column is
/// `Option<bool>` because it comes from a `LEFT JOIN` against the parent's
/// own `thread_summaries` row — `None` either when the child has no parent
/// (immediately filtered out below) or when that parent row is missing
/// (corruption, no safe default).
type ChildSummaryRow = (
    Option<Uuid>,
    bool,
    Option<String>,
    Option<String>,
    bool,
    Option<bool>,
);

impl EventBus {
    /// Send a ChildrenCountChanged transient event to the parent thread's SSE channel.
    /// `aggregate` carries any other projection changes (e.g. archive_state) the
    /// caller made before emitting — the frontend overlays it onto thread.meta.
    pub(super) fn send_children_count_event(
        &self,
        parent_id: Uuid,
        active: i64,
        total: i64,
        aggregate: Option<crate::core::store::ThreadAggregate>,
    ) {
        let _ = self.event_tx.send(EmittedEvent {
            event_id: Uuid::new_v4(),
            seq: None,
            created: Utc::now(),
            typed: BusEvent::Thread {
                thread_id: parent_id,
                event: ThreadEvent::ChildrenCountChanged { active, total },
                meta: EventMeta::default(),
            },
            aggregate,
        });
    }

    /// Query children counts from DB and broadcast to the parent thread's SSE channel.
    pub(super) async fn broadcast_children_count(&self, parent_id: Uuid) {
        let counts: Option<(i64, i64)> = match sqlx::query_as(
            "SELECT active_children_count::bigint, total_children_count::bigint FROM thread_summaries WHERE thread_id = $1"
        )
        .bind(parent_id)
        .fetch_optional(&self.pool)
        .await {
            Ok(row) => row,
            Err(e) => {
                crate::log!("[EventBus] Failed to query children counts for {}: {}", parent_id, e);
                return;
            }
        };
        if let Some((active, total)) = counts {
            self.send_children_count_event(parent_id, active, total, None);
        }
    }

    /// Handle parent notification when a child thread emits a terminal event.
    /// Decrements the parent's `active_children_count` and, for completion events,
    /// sends a callback message with results.
    pub(super) async fn notify_parent_if_child(&self, child_thread_id: Uuid, event: &ThreadEvent) {
        // Cancel = user-driven, terminal. Abort splits on `AbortCause::is_transient`:
        // EngineShutdown / RecoveryAfterRestart are mid-retry (no decrement, no
        // callback — the resumed child's eventual idle would be orphaned);
        // SafetyNet / ProcessKilled / Unknown are terminal (decrement so the
        // parent doesn't display as Active forever, but no card — the user
        // already sees the child's error state). Same `is_transient` shape as
        // `SessionEnded { reason }` below.
        //
        // Cancel splits on `CancelCause` for the same reason. A
        // `SupersededByFollowup` is the mid-turn redirect `arm_followup_redirect`
        // arms when a follow-up lands on a live Codex turn: the caller steered,
        // they did not abandon (`thread_events/cause.rs`), and the child runs
        // the redirected turn immediately after. Reporting it would wake the
        // parent with a false "child canceled" card for work that is still
        // running, and the parent may spawn a replacement. The redirected
        // turn's own terminal is the real report. Dropping out of the terminal
        // set here leaves the in-tx `reconcile_parent_active_children_count`
        // (which runs in the `ResponseCanceled` projection arm, cause-agnostic)
        // as the only thing that fires, so no counter drifts.
        let is_terminal = match event {
            ThreadEvent::CodingAgentIdled { .. }
            | ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseFailed { .. } => true,
            ThreadEvent::ResponseCanceled { cause, .. } => {
                !matches!(cause, CancelCause::SupersededByFollowup)
            }
            ThreadEvent::ResponseAborted { cause, .. } => !cause.is_transient(),
            ThreadEvent::SessionEnded { reason } => !reason.is_transient(),
            _ => false,
        };
        if !is_terminal {
            return;
        }

        // Look up parent, child info, CC status, and whether the parent
        // callback for this run is still pending. Self-join to pick up the parent's
        // own `is_coding_agent` in the same roundtrip — the FanOut consumer
        // needs it to pick the CC vs chat routing fork, and skipping the
        // join would force a second query per child completion.
        let row: Option<ChildSummaryRow> = match sqlx::query_as::<_, ChildSummaryRow>(
            "SELECT c.parent_thread_id, c.is_coding_agent, c.title, c.first_message, \
                    c.parent_callback_pending, p.is_coding_agent \
             FROM thread_summaries c \
             LEFT JOIN thread_summaries p ON p.thread_id = c.parent_thread_id \
             WHERE c.thread_id = $1",
        )
        .bind(child_thread_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(row)) => Some(row),
            Ok(None) => return,
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to look up parent for child {}: {}",
                    child_thread_id,
                    e
                );
                return;
            }
        };

        let Some((
            Some(parent_id),
            is_coding_agent,
            title,
            first_msg,
            parent_callback_pending,
            parent_is_coding_agent,
        )) = row
        else {
            return;
        };

        // CC threads can emit CodingAgentIdled multiple times (initial work,
        // auto-harden, background agents). Only process the first one —
        // subsequent idles should not decrement the counter again or send
        // duplicate callbacks to the parent. Reads as "this child owes its
        // parent nothing, so swallow the extra idle": the marker is set again
        // by the next start event, so a REAL new turn is never swallowed.
        if is_coding_agent
            && !parent_callback_pending
            && matches!(event, ThreadEvent::CodingAgentIdled { .. })
        {
            return;
        }

        // Coding-agent sessions can terminate without ever emitting CodingAgentIdled or
        // SessionEnded — e.g. the user cancels and the session sits archived,
        // leaving only ResponseCanceled (or a terminal-cause ResponseAborted
        // from a SafetyNet / ProcessKilled crash) as the signal. The
        // `parent_callback_pending` guard collapses multiple terminal events
        // for the same child to a single decrement. ResponseFailed sits in
        // the gated set too: a failing coding-agent turn typically emits ResponseFailed
        // immediately followed by CodingAgentIdled, and ResponseFailed's
        // `should_callback` branch clears the pending marker, so the follow-up
        // Idled early-returns via the dedup guard and never decrements. Add
        // ResponseFailed here so the decrement happens on the ResponseFailed
        // itself; otherwise the parent's count leaks by 1 per failed turn
        // and the parent pulses as "waiting for children" forever. Transient
        // aborts (engine shutdown, recovery) already early-returned via
        // `is_terminal`.
        let should_decrement = if is_coding_agent {
            matches!(event, ThreadEvent::CodingAgentIdled { .. })
                || (parent_callback_pending
                    && matches!(
                        event,
                        ThreadEvent::SessionEnded { .. }
                            | ThreadEvent::ResponseCanceled { .. }
                            | ThreadEvent::ResponseAborted { .. }
                            | ThreadEvent::ResponseFailed { .. }
                    ))
        } else {
            matches!(
                event,
                ThreadEvent::ResponseGenerated { .. }
                    | ThreadEvent::ResponseFailed { .. }
                    | ThreadEvent::ResponseCanceled { .. }
                    | ThreadEvent::ResponseAborted { .. }
                    | ThreadEvent::SessionEnded { .. }
            )
        };
        // Completion events trigger a callback to the parent (typed
        // ChildThreadCompleted on the parent thread) and surface the parent
        // to inbox. ResponseCanceled is included so the parent sees a
        // "Canceled" card and the LLM learns the child was stopped, except for
        // a `SupersededByFollowup` redirect, which never reaches here (the
        // `is_terminal` split above already returned).
        // ResponseAborted is NOT — the user already sees the child's error
        // state (SafetyNet/ProcessKilled), and engine-shutdown aborts are
        // transient (and were filtered out above). For coding-agent children,
        // SessionEnded also counts when the run's callback is still pending:
        // handles coding-agent sessions that end without ever idling.
        let should_callback = matches!(
            (is_coding_agent, event),
            (true, ThreadEvent::CodingAgentIdled { .. })
                | (false, ThreadEvent::ResponseGenerated { .. })
                | (_, ThreadEvent::ResponseFailed { .. })
                | (_, ThreadEvent::ResponseCanceled { .. })
        ) || (is_coding_agent
            && parent_callback_pending
            && matches!(event, ThreadEvent::SessionEnded { .. }));

        // Decrement-only paths must still clear the marker or a follow-up event
        // (CodingAgentIdled, SessionEnded) re-decrements via the
        // `parent_callback_pending` gate above. The should_callback path clears
        // in-tx via the ChildThreadCompleted projection arm; abort never
        // emits a typed event, so clear here directly.
        // Non-CC chat children emit exactly one terminator per request (the
        // agentic loop's `has_terminator_for` guard), so they need no marker; CC
        // children can have multiple terminal events for the same turn.
        let clear_callback_for_terminal_abort = should_decrement
            && is_coding_agent
            && matches!(event, ThreadEvent::ResponseAborted { .. });

        // The decrement of parent's `active_children_count` runs IN-TX via
        // `reconcile_parent_active_children_count` in `update_thread_projection`
        // for every terminal event arm, and the same projection step pushes
        // the parent onto `extra_ancestors` so the post-commit ancestor loop
        // broadcasts a `ChildrenCountChanged` with the correct value. The
        // only work left here is the archive-state flip + its broadcast
        // when `should_callback` is true — gated accordingly. Pure
        // `should_decrement && !should_callback` paths (CC terminal abort,
        // non-CC ResponseAborted/SessionEnded) used to land here only for
        // the decrement; the in-tx reconcile + in-tx broadcast now cover it.
        if should_callback {
            self.update_parent_after_child_terminal(parent_id).await;
        }

        if clear_callback_for_terminal_abort {
            self.clear_pending_parent_callback(child_thread_id).await;
        }

        if !should_callback {
            return;
        }

        let label = title
            .or_else(|| first_msg.map(|m| m.chars().take(80).collect()))
            .unwrap_or_else(|| "unknown task".into());

        // Fetch the child thread's last response text — becomes the
        // `summary` field on the typed event (and the failure-error path
        // overrides it below). 2000-char cap mirrors the previous prose
        // path's truncation.
        let last_response: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseGenerated' \
             AND payload->>'text' IS NOT NULL AND payload->>'text' != '' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(child_thread_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[FanOut] Failed to fetch child response for {}: {}",
                child_thread_id,
                e
            );
            None
        });

        // Per-status caps differ deliberately: success / no-changes / canceled
        // summaries come from a real ResponseGenerated.text (or partial text)
        // the orchestrator may want most of (2000 chars). Failure summaries
        // come from ResponseFailed.error which is often a panic / stack trace
        // and should never dominate the parent's context — re-cap to 200 chars
        // (matching the pre-Phase-4 prose path) before truncation.
        const SUCCESS_SUMMARY_CAP: usize = 2000;
        const FAILURE_SUMMARY_CAP: usize = 200;
        let (status, summary, cap) = match event {
            ThreadEvent::CodingAgentIdled {
                has_changes: true, ..
            } => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::CodingAgentIdled {
                has_changes: false, ..
            } => (
                ChildCompletionStatus::NoChanges,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::ResponseGenerated { .. } => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::ResponseFailed { error } => (
                ChildCompletionStatus::Failure,
                error.clone(),
                FAILURE_SUMMARY_CAP,
            ),
            // User-driven cancel — surface to parent so the LLM learns the
            // child was stopped (and the UI shows a Canceled card). Pull the
            // partial-stream text from ResponseCanceled itself; falling back
            // to last_response would surface a *prior* turn's text and read
            // as if the canceled turn had completed.
            ThreadEvent::ResponseCanceled { text, .. } => (
                ChildCompletionStatus::Canceled,
                text.clone(),
                SUCCESS_SUMMARY_CAP,
            ),
            // Defensive: any other terminal event slipping through still
            // produces a typed completion so the parent isn't left waiting.
            _ => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
        };

        let summary = {
            if summary.len() > cap {
                let cut = summary.floor_char_boundary(cap);
                let mut s = summary[..cut].to_string();
                s.push_str("… (truncated)");
                s
            } else {
                summary
            }
        };

        // Look up which proposed changes the child left in `pending` state.
        // CC chats that ended with `has_changes: true` typically have one;
        // chat children and `no_changes` idles have zero. Failures are
        // reported with `pending_change_ids = []` regardless because the
        // worktree state isn't reviewable.
        let pending_change_ids: Vec<String> = if matches!(status, ChildCompletionStatus::Failure) {
            Vec::new()
        } else {
            match self
                .changes_projection
                .pending_for_thread(child_thread_id)
                .await
            {
                Ok(v) => v.into_iter().map(|c| c.id.to_string()).collect(),
                Err(e) => {
                    crate::log!(
                        "[EventBus] notify_parent_if_child: pending_for_thread({}): {} — \
                         emitting ChildThreadCompleted with no pending_change_ids",
                        child_thread_id,
                        e
                    );
                    Vec::new()
                }
            }
        };

        // Emit the typed source-of-truth event onto the parent thread. The
        // `parent_callback_pending` marker is cleared by THIS emit's projection
        // arm (see ChildThreadCompleted in `update_thread_projection`), in
        // the same transaction as the event INSERT — so the event and the
        // marker cannot disagree. The previous shape (emit, then a separate
        // post-emit UPDATE) left a crash window where the event committed
        // but the marker didn't, and the next terminal event handed the
        // parent a duplicate completion card. `EventMeta::NONE` because this
        // fan-in is engine orchestration, not user/agent actor.
        let typed_event = ThreadEvent::ChildThreadCompleted {
            child_thread_id,
            child_thread_title: Some(label.clone()),
            status,
            summary,
            pending_change_ids,
        };
        // Box::pin here because `emit_or_log → emit → notify_parent_if_child`
        // is the recursion the compiler flags — the emitted ChildThreadCompleted
        // for the parent will itself walk back through `notify_parent_if_child`
        // (which immediately exits because ChildThreadCompleted isn't a
        // terminal event), but the type-level cycle still needs indirection.
        //
        // If the typed-event emit failed, we MUST NOT mark the callback as
        // sent or fire the wake-up kick — telling the parent's resume path
        // to attribute its response to a non-persisted event would have the
        // parent's UI displaying a phantom card. Returning here means the
        // next terminal event (or recovery sweep) gets another shot at the
        // typed emit; for chat children whose ResponseGenerated fires
        // exactly once that means a permanent miss, accepted as the lesser
        // evil vs. the phantom-event case.
        let emit_result = match Box::pin(self.emit(BusEvent::Thread {
            thread_id: parent_id,
            event: typed_event,
            meta: EventMeta::NONE,
        }))
        .await
        {
            Ok(Some(r)) => r,
            Ok(None) => {
                crate::log!(
                    "[FanOut] ChildThreadCompleted emit returned None for parent {}; \
                     skipping wake-up kick — see notify_parent_if_child error path.",
                    parent_id
                );
                return;
            }
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to emit ChildThreadCompleted for parent {}: {}; \
                     skipping wake-up kick — driving the parent against a non-persisted \
                     event would attribute its response to a phantom card. Next terminal \
                     event (if any) retries; for chat children with one-shot \
                     ResponseGenerated this means a permanent miss.",
                    parent_id,
                    e
                );
                return;
            }
        };

        // Defensive: `parent_is_coding_agent` should always be `Some` here
        // (the parent thread row must exist for its child to have spawned),
        // so a `None` is a serious state corruption — skip the kick rather
        // than guess. The projection arm already cleared the child's marker
        // atomically with the emit, which would consume the callback and block
        // every later terminal's retry through the `parent_callback_pending`
        // gate. The card is persisted but the wake never reached
        // `parent_callback_tx`, so the parent callback genuinely IS still
        // pending: write it back so the next terminal event re-fires the fan-in
        // and gets another shot at the kick. Cost while the corruption persists:
        // one duplicate persisted ChildThreadCompleted per terminal event
        // (unbounded across a long session, N duplicate cards if the parent
        // row is later repaired) — accepted as the lesser evil vs. a
        // permanently silent parent.
        let Some(parent_is_cc) = parent_is_coding_agent else {
            crate::log!(
                "[FanOut] Parent {} for child {} missing from thread_summaries — \
                 skipping wake-up kick; typed event already persisted. Leaving \
                 the parent callback pending so the next terminal event retries.",
                parent_id,
                child_thread_id
            );
            if let Err(e) = sqlx::query(
                "UPDATE thread_summaries SET parent_callback_pending = TRUE WHERE thread_id = $1",
            )
            .bind(child_thread_id)
            .execute(&self.pool)
            .await
            {
                crate::log!(
                    "[FanOut] Failed to re-mark the pending callback for child {}: {}",
                    child_thread_id,
                    e
                );
            }
            return;
        };
        self.send_parent_callback(
            parent_id,
            child_thread_id,
            emit_result.event_id,
            parent_is_cc,
        );
    }

    /// Surface the parent to inbox on a child completion that warrants user
    /// attention (`should_callback`), then broadcast the parent's updated
    /// aggregate so subscribers pick up the archive flip in the same
    /// envelope as any other counter changes from the in-tx reconcile.
    ///
    /// Counter mutation (`active_children_count`) used to live here too as
    /// `GREATEST(0, active_children_count - 1)`, but that double-decremented
    /// the parent when the in-tx `reconcile_parent_active_children_count`
    /// had already recomputed the column from ground truth. The reconcile
    /// in `update_thread_projection` is now the sole writer; this function
    /// only handles the archive flip + the SSE rebroadcast.
    async fn update_parent_after_child_terminal(&self, parent_id: Uuid) {
        let row: Option<(i64, i64)> = match sqlx::query_as(
            "UPDATE thread_summaries SET archive_state = $2 \
             WHERE thread_id = $1 \
             RETURNING active_children_count::bigint, total_children_count::bigint",
        )
        .bind(parent_id)
        .bind(ArchiveState::Inbox.as_str())
        .fetch_optional(&self.pool)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                crate::log!(
                    "[FanOut] Failed to update parent {} after child terminal: {}",
                    parent_id,
                    e
                );
                return;
            }
        };
        let Some((active, total)) = row else { return };
        let aggregate =
            match crate::core::store::fetch_thread_aggregate(&self.pool, parent_id).await {
                Ok(agg) => agg,
                Err(e) => {
                    crate::log!(
                        "[FanOut] Failed to fetch aggregate for parent {}: {}",
                        parent_id,
                        e
                    );
                    None
                }
            };
        self.send_children_count_event(parent_id, active, total, aggregate);
    }

    /// Settle the child's `parent_callback_pending` marker on a decrement-only
    /// terminal (a coding-agent child whose terminal is a crash-class
    /// `ResponseAborted`). Nothing further is owed to the parent: the counter
    /// already came down through the in-tx reconcile, no card is sent by
    /// design, and the user is looking at the child's error state. The child
    /// keeps FALSE until a start event sets it again.
    async fn clear_pending_parent_callback(&self, child_thread_id: Uuid) {
        if let Err(e) = sqlx::query(
            "UPDATE thread_summaries SET parent_callback_pending = FALSE WHERE thread_id = $1",
        )
        .bind(child_thread_id)
        .execute(&self.pool)
        .await
        {
            crate::log!(
                "[FanOut] Failed to clear the pending callback for child {}: {}",
                child_thread_id,
                e
            );
        }
    }

    /// Boot-recovery sweep that re-derives parent-resume wakes lost to an engine
    /// restart (ADR 0011, weakness B1). The in-memory `ParentCallback` channel is
    /// recreated empty on restart, so a child that completed while the engine was
    /// down — or whose wake was queued but not yet consumed when it died — leaves
    /// a persisted `ChildThreadCompleted` on the parent with no resume ever fired.
    /// Without this sweep the fan-in strands permanently.
    ///
    /// Selection: every parent whose **latest persisted event is a
    /// `ChildThreadCompleted`** — nothing came after the completion card, so the
    /// parent never reacted. The moment a parent resumes it emits a fresh terminal
    /// event with a higher sequence, so a processed completion is no longer the
    /// latest event and is skipped; a resume that died mid-flight leaves a
    /// `SessionStarted` / streamed token as the latest event and is handled by the
    /// CC auto-resume recovery instead, not re-fired here. This makes the sweep
    /// idempotent across boots via the event-id anchor, with no double-handling.
    ///
    /// Each candidate is re-injected onto the same `parent_callback_tx` the live
    /// fan-in uses, so the already-running listener drains it through the exact
    /// same `notify_parent_of_child_completion` path — the recovery duplicates no
    /// resume logic, it only re-delivers the lost wake. Returns the number of
    /// wakes re-fired (for the boot log). Mirrors
    /// `propose_held_back_changes_on_startup`: the persisted event is the source
    /// of truth, the in-memory wake is a cache rebuilt from it.
    pub async fn refire_unprocessed_child_completions(&self) -> usize {
        // `e.aggregate_id` is the parent thread id (text); `payload->>'child_thread_id'`
        // is the child's uuid string. The JOIN drops parents whose summary row is
        // gone (can't resume a parent with no row); `state IS DISTINCT FROM
        // 'discarded'` (NULL-safe) skips a parent the user threw away while still
        // selecting legacy NULL-state rows. The NOT EXISTS makes the card the
        // thread's last word — scoped to `aggregate = 'thread'` to match
        // `lookup_last_activity` and never let a same-id non-thread event suppress
        // a real fan-in. See the selection rationale above.
        //
        // The OUTER `e.aggregate = 'thread'` is load-bearing for a second reason.
        // On a DOMAIN event `aggregate_id` holds the event TYPE NAME, not a uuid,
        // and `POST /api/v1/events/emit` accepts any type name that is not a
        // reserved `SystemEvent`. `ChildThreadCompleted` is a `ThreadEvent` name,
        // so an app UI (untrusted, per that handler's own comment) can persist a
        // row whose `aggregate_id` is the literal string. Events are append-only,
        // so an unscoped `e.aggregate_id::uuid` JOIN would then fail this sweep
        // with `invalid input syntax for type uuid` on EVERY later boot, and every
        // child-completion wake lost to a restart would strand its parent forever.
        let rows: Vec<(Uuid, String, Option<String>, bool)> = match sqlx::query_as(
            "SELECT e.id, e.aggregate_id, e.payload->>'child_thread_id', p.is_coding_agent \
             FROM events e \
             JOIN thread_summaries p ON p.thread_id = e.aggregate_id::uuid \
             WHERE e.aggregate = 'thread' \
               AND e.event_type = 'ChildThreadCompleted' \
               AND p.state IS DISTINCT FROM 'discarded' \
               AND NOT EXISTS ( \
                 SELECT 1 FROM events later \
                 WHERE later.aggregate = 'thread' \
                   AND later.aggregate_id = e.aggregate_id \
                   AND later.sequence > e.sequence \
               )",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                crate::log!(
                    "[FanOut] refire_unprocessed_child_completions query failed: {} — \
                     skipping recovery sweep this boot",
                    e
                );
                return 0;
            }
        };

        let mut refired = 0usize;
        for (event_id, parent_str, child_str, parent_is_coding_agent) in rows {
            let (Ok(parent_id), Some(Ok(child_id))) = (
                parent_str.parse::<Uuid>(),
                child_str.as_deref().map(str::parse::<Uuid>),
            ) else {
                crate::log!(
                    "[FanOut] Skipping unprocessed ChildThreadCompleted {} — \
                     malformed parent ({:?}) or child ({:?}) id",
                    event_id,
                    parent_str,
                    child_str
                );
                continue;
            };
            crate::log!(
                "[FanOut] Re-firing lost parent wake: child {} completed, parent {} never resumed (event {})",
                child_id,
                parent_id,
                event_id
            );
            self.send_parent_callback(parent_id, child_id, event_id, parent_is_coding_agent);
            refired += 1;
        }
        if refired > 0 {
            crate::log!(
                "[FanOut] Re-fired {} unprocessed child-completion wake(s) lost to engine restart",
                refired
            );
        }
        refired
    }

    fn send_parent_callback(
        &self,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
        parent_is_coding_agent: bool,
    ) {
        if let Err(e) = self.parent_callback_tx.send(ParentCallback {
            parent_thread_id,
            child_thread_id,
            child_completed_event_id,
            parent_is_coding_agent,
        }) {
            crate::log!(
                "[FanOut] Failed to send parent callback for child {}: {}",
                child_thread_id,
                e
            );
        }
    }
}
