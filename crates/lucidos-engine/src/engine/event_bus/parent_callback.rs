//! Child→parent fan-in.
//!
//! When a child thread reaches a terminal event, [`EventBus::emit`]'s PostCommit
//! phase drives these methods to decrement the parent's `active_children_count`,
//! surface the parent to inbox, and emit the typed `ChildThreadCompleted` onto
//! the parent thread.

use chrono::Utc;
use uuid::Uuid;

use super::{BusEvent, EmittedEvent, EventBus, ParentCallback};
use crate::engine::thread_events::{
    CancelCause, ChildCompletionStatus, EventMeta, EventWaitCancelCause, ThreadEvent,
};
use crate::engine::thread_lifecycle::ArchiveState;

/// What the fan-in reads about a child, in one self-joined query.
///
/// `parent_is_coding_agent` comes from a `LEFT JOIN` on the parent's own row.
/// It is `None` when the child has no parent, or when that parent row is
/// missing (corruption, no safe default).
#[derive(sqlx::FromRow)]
struct ChildRow {
    parent_thread_id: Option<Uuid>,
    is_coding_agent: bool,
    title: Option<String>,
    first_message: Option<String>,
    parent_callback_pending: bool,
    is_stopped_child: bool,
    holds_live_wait: bool,
    status: String,
    coding_agent_proposed: bool,
    parent_is_coding_agent: Option<bool>,
}

const CHILD_ROW_SQL: &str = "SELECT c.parent_thread_id, c.is_coding_agent, c.title, \
            c.first_message, c.parent_callback_pending, c.is_stopped_child, \
            c.live_event_wait_count > 0 AS holds_live_wait, c.status, \
            c.coding_agent_proposed, p.is_coding_agent AS parent_is_coding_agent \
     FROM thread_summaries c \
     LEFT JOIN thread_summaries p ON p.thread_id = c.parent_thread_id \
     WHERE c.thread_id = $1";

/// Per-status summary caps. Success, no-changes and canceled summaries come
/// from a real response the orchestrator may want most of. A failure summary
/// is often a panic or a stack trace and must never dominate the parent's
/// context.
const SUCCESS_SUMMARY_CAP: usize = 2000;
const FAILURE_SUMMARY_CAP: usize = 200;

/// Cut `summary` to `cap` bytes on a char boundary, marking the cut.
fn cap_summary(summary: String, cap: usize) -> String {
    if summary.len() <= cap {
        return summary;
    }
    let cut = summary.floor_char_boundary(cap);
    format!("{}… (truncated)", &summary[..cut])
}

/// Whether a child is mid-turn, by the one in-flight definition the counters
/// use. A child mid-turn is not owed a settle: its own terminal will report.
fn in_flight(status: &str) -> bool {
    crate::core::store::active_thread_statuses().contains(&status)
}

/// The name a completion card gives the child: its title, else the start of
/// its first message.
fn child_label(title: Option<String>, first_message: Option<String>) -> String {
    title
        .or_else(|| first_message.map(|m| m.chars().take(80).collect()))
        .unwrap_or_else(|| "unknown task".into())
}

/// Which user act ended a child that still owed its parent a card: a
/// *stopped child* (ADR 0252) or one that ended a turn holding an event wait
/// (ADR 0254). Each becomes the summary of the canceled card, so the parent
/// reads what the user actually did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChildSettle {
    Archived,
    Discarded,
    Deleted,
}

impl ChildSettle {
    fn summary(self) -> &'static str {
        match self {
            Self::Archived => "The user archived this child thread. Its work is not continuing.",
            Self::Discarded => {
                "The user discarded this child thread's change. Its work is not continuing."
            }
            Self::Deleted => "The user deleted this child thread. Its work is not continuing.",
        }
    }

    /// Whether this act ends the child's event waits. Archive and delete do.
    /// A discard does not, so a waiting child wakes again and its turn reports.
    fn ends_waits(self) -> bool {
        !matches!(self, Self::Discarded)
    }
}

/// A canceled card a settle decided is owed, read while the child's row still
/// exists. Delivered separately, because a delete must read before it deletes
/// and deliver after it commits.
pub(crate) struct OwedChildCard(ChildCompletion);

/// One `ChildThreadCompleted` owed to a parent, ready to deliver. The terminal
/// fan-in, a settle, and the end of a child's last event wait each build one.
struct ChildCompletion {
    child_thread_id: Uuid,
    parent_id: Uuid,
    parent_is_coding_agent: Option<bool>,
    label: String,
    status: ChildCompletionStatus,
    summary: String,
    terminal_event_id: Option<Uuid>,
}

/// What a wake changed on the parent's row, so a caller that then fails to
/// drive a turn can put it back exactly.
///
/// `revived_at` is a fencing token, not bookkeeping. The rollback matches on
/// it. Any revive landing between the wake and the failure re-stamps the row,
/// so the rollback then no-ops instead of settling a live turn.
/// Both previous values are carried so the rollback is a true restore. Keeping
/// the new stamp would sort the thread by a revive that was undone. It would
/// also break the fence for a second wake unwinding behind this one.
///
/// **Known bound: two failed wakes unwinding OLDEST first do not fully
/// settle.** The older rollback no-ops, its stamp being gone, and the newer
/// one restores `running`. Reaching it needs two children of one parent
/// finishing together AND both callbacks failing. That means shutdown in
/// practice, and `main.rs`'s boot reset clears an orphaned `running` on the
/// way back up. Closing it properly means asking whether a turn is really in
/// flight, which only the engine can answer: the bus cannot see
/// `active_threads`.
pub(super) struct ParentWake {
    pub(super) prev_status: String,
    /// Nullable: a thread that has never been revived carries no stamp.
    pub(super) prev_revived_at: Option<chrono::DateTime<Utc>>,
    pub(super) revived_at: chrono::DateTime<Utc>,
}

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
            depth: super::thread_emit_depth(parent_id),
            emitting_trigger_id: crate::scheduler::user_tasks::current_trigger_id(),
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
    ///
    /// `terminal_event_id` is `event`'s own row id. It travels to the parent on
    /// the [`ParentCallback`], where the stand-down gate needs it. `None` when
    /// the caller has no row to name: [`Self::announce_withheld_completion`]
    /// replays a terminal whose own fan-in already stood down, and the gate then
    /// abstains, which is the fail-open side.
    pub(super) async fn notify_parent_if_child(
        &self,
        child_thread_id: Uuid,
        terminal_event_id: Option<Uuid>,
        event: &ThreadEvent,
    ) {
        // Not a terminal, and still sometimes the moment a card falls due:
        // see `announce_when_last_wait_ends`.
        if let ThreadEvent::EventWaitCanceled { cause, .. } = event {
            self.announce_when_last_wait_ends(child_thread_id, *cause)
                .await;
            return;
        }

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

        // **A terminal the engine is about to auto-resume is not a completion.**
        // The child is still working, and the resume lands seconds later. A card
        // here tells the parent the work is lost, and a parent that believes it
        // spawns a duplicate onto the same files. Withhold, and let the REAL
        // terminal report.
        //
        // Below the `is_terminal` gate on purpose, and not above it. A hold can
        // only ever concern a terminal, while this function runs in PostCommit
        // for EVERY persisted thread event. Asking first would take a global
        // mutex per streamed token and log a withholding line for events that
        // carry no card.
        //
        // `parent_callback_pending` is deliberately left alone, so the next
        // terminal still fires. Standing down before the decrement gate is fine
        // too: the counter comes down through the in-tx reconcile either way,
        // and `ContinuationRequested` puts it back.
        if self.auto_resume_holds.is_held(child_thread_id) {
            crate::log!(
                "[FanOut] Child {} ended on a terminal the engine is auto-resuming: \
                 withholding the parent's completion card until the real terminal",
                child_thread_id
            );
            return;
        }

        let Some(ChildRow {
            parent_thread_id: Some(parent_id),
            is_coding_agent,
            title,
            first_message: first_msg,
            parent_callback_pending,
            is_stopped_child,
            holds_live_wait,
            parent_is_coding_agent,
            ..
        }) = self.load_child_row(child_thread_id).await
        else {
            return;
        };

        // **A user Stop pauses a child; it does not finish it** (ADR 0252).
        // The child is alive and one message continues it, so the parent gets
        // a note and no turn. A woken parent acts, and here it would act on
        // work that is still running.
        //
        // `is_stopped_child` was set in-tx by the cancel's own projection arm,
        // only when this turn was still owed a card and a person pressed Stop.
        // A Stop the parent is owed nothing for returns in silence. A cancel an
        // agent issued is not a Stop of the user's: it falls through and
        // reports a canceled card, as it always did.
        //
        // While the child stays stopped, no terminal speaks for it. The idle
        // that follows the Stop is the one that matters: the marker is still
        // set, so without this it would reach the parent as a "completed"
        // card. What settles a stopped child is a start event, which clears
        // the flag and lets its turn report, or `settle_child`.
        if let ThreadEvent::ResponseCanceled {
            cause: CancelCause::UserStop,
            ..
        } = event
        {
            if is_stopped_child {
                self.announce_stopped_child(
                    parent_id,
                    child_thread_id,
                    child_label(title, first_msg),
                )
                .await;
                return;
            }
            if !parent_callback_pending {
                return;
            }
        } else if is_stopped_child {
            return;
        }

        // **A child holding a live event wait has not finished** (ADR 0254).
        // Arming a wait and ending the turn is the event-wait contract, and
        // the wait wakes the child for another turn. So a turn that ends
        // holding one reports nothing, and the marker stays set for the turn
        // the wait wakes. A failed turn still reports: that is loud and worth
        // knowing whatever comes next. A cancel that ended the child reports
        // too.
        if holds_live_wait
            && matches!(
                event,
                ThreadEvent::CodingAgentIdled { .. }
                    | ThreadEvent::ResponseGenerated { .. }
                    | ThreadEvent::SessionEnded { .. }
            )
        {
            crate::log!(
                "[FanOut] Child {} ended a turn holding an event wait: no card until the \
                 turn the wait wakes ends",
                child_thread_id
            );
            return;
        }

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
        // to inbox. ResponseCanceled is included for a cancel that is not a
        // person's Stop: an agent cancelling its own child, or a `UserAction`.
        // A person's Stop and a `SupersededByFollowup` redirect returned above.
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
        if clear_callback_for_terminal_abort {
            self.clear_pending_parent_callback(child_thread_id).await;
        }

        if !should_callback {
            return;
        }

        let label = child_label(title, first_msg);

        // The child's last response text becomes the `summary` field on the
        // typed event. The failure path overrides it with the error.
        let last_response = self.last_response_text(child_thread_id).await;

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
            // A cancel that ended the child. Pull the
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

        self.deliver_child_completion(ChildCompletion {
            child_thread_id,
            parent_id,
            parent_is_coding_agent,
            label,
            status,
            summary: cap_summary(summary, cap),
            terminal_event_id,
        })
        .await;
    }

    /// Read [`ChildRow`] for `child_thread_id`. `None` when the row is gone, or
    /// when the read failed, which is logged.
    async fn load_child_row(&self, child_thread_id: Uuid) -> Option<ChildRow> {
        sqlx::query_as::<_, ChildRow>(CHILD_ROW_SQL)
            .bind(child_thread_id)
            .fetch_optional(&self.pool)
            .await
            .unwrap_or_else(|e| {
                crate::log!(
                    "[FanOut] Failed to look up child {}: {}",
                    child_thread_id,
                    e
                );
                None
            })
    }

    /// The text of `child_thread_id`'s newest `ResponseGenerated`, if any.
    async fn last_response_text(&self, child_thread_id: Uuid) -> Option<String> {
        sqlx::query_scalar(
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
        })
    }

    /// Deliver the card a waiting child held back, once its last event wait
    /// ends without waking it (ADR 0254).
    ///
    /// A delivery or an expiry wakes the child, and that turn reports. Stop
    /// waiting and an agent standing a wait down wake nothing, so without this
    /// the parent would never hear. An archive or a discard of the thread is
    /// the settle's business: see `settle_child`.
    ///
    /// Only an idle child still owed a card qualifies. A running child's own
    /// terminal reports, and a stopped child waits for the user.
    async fn announce_when_last_wait_ends(
        &self,
        child_thread_id: Uuid,
        cause: EventWaitCancelCause,
    ) {
        if !matches!(
            cause,
            EventWaitCancelCause::UserStop | EventWaitCancelCause::AgentStandDown
        ) {
            return;
        }
        // Idle only. A failed or paused child still owes its parent a card,
        // and the next resume or the settle carries it with the real status.
        let Some(ChildRow {
            parent_thread_id: Some(parent_id),
            is_coding_agent,
            title,
            first_message: first_msg,
            parent_callback_pending: true,
            is_stopped_child: false,
            holds_live_wait: false,
            status,
            coding_agent_proposed: proposed,
            parent_is_coding_agent,
        }) = self.load_child_row(child_thread_id).await
        else {
            return;
        };
        if status != "idle" {
            return;
        }
        crate::log!(
            "[FanOut] Child {}'s last event wait ended without waking it: sending its \
             parent {} the card it held back",
            child_thread_id,
            parent_id
        );
        let summary = self
            .last_response_text(child_thread_id)
            .await
            .unwrap_or_default();
        self.deliver_child_completion(ChildCompletion {
            child_thread_id,
            parent_id,
            parent_is_coding_agent,
            label: child_label(title, first_msg),
            status: if is_coding_agent && !proposed {
                ChildCompletionStatus::NoChanges
            } else {
                ChildCompletionStatus::Success
            },
            summary: cap_summary(summary, SUCCESS_SUMMARY_CAP),
            terminal_event_id: None,
        })
        .await;
    }

    /// Record on the parent that a user Stop paused one of its children
    /// (ADR 0252). No wake, no status write and no callback: the parent stays
    /// exactly as it was, and reads the note on its next turn.
    async fn announce_stopped_child(&self, parent_id: Uuid, child_thread_id: Uuid, label: String) {
        // Box::pin for the same recursion `deliver_child_completion` breaks.
        Box::pin(self.emit_or_log(
            BusEvent::Thread {
                thread_id: parent_id,
                event: ThreadEvent::ChildThreadStopped {
                    child_thread_id,
                    child_thread_title: Some(label),
                },
                meta: EventMeta::NONE,
            },
            "[FanOut] ChildThreadStopped",
        ))
        .await;
    }

    /// Send the canceled card a child still owed its parent, because the user
    /// archived, discarded or deleted it (ADR 0252, ADR 0254). A no-op for any other thread, so a caller need not check first.
    ///
    /// The caller decides WHICH thread the user acted on. A cascading archive
    /// must pass only its root: a grandchild archived along with its parent
    /// owes that parent nothing, and a card would un-archive it.
    pub(crate) async fn settle_child(&self, child_thread_id: Uuid, settled_by: ChildSettle) {
        if let Some(card) = self.owed_child_card(child_thread_id, settled_by).await {
            self.deliver_owed_child_card(card).await;
        }
    }

    /// The canceled card `child_thread_id` owes its parent if the user ends it
    /// by `settled_by`, or `None` when it owes nothing.
    ///
    /// Owed means the marker is still set on a child not in flight: a stopped
    /// child, one that ended a turn holding an event wait, or one a crash left
    /// owed. A child still holding a wait owes nothing to an act that leaves
    /// its waits alone, because it wakes again and its turn reports.
    pub(crate) async fn owed_child_card(
        &self,
        child_thread_id: Uuid,
        settled_by: ChildSettle,
    ) -> Option<OwedChildCard> {
        let ChildRow {
            parent_thread_id: Some(parent_id),
            title,
            first_message,
            parent_callback_pending: true,
            holds_live_wait,
            status,
            parent_is_coding_agent,
            ..
        } = self.load_child_row(child_thread_id).await?
        else {
            return None;
        };
        if in_flight(&status) || (holds_live_wait && !settled_by.ends_waits()) {
            return None;
        }
        Some(OwedChildCard(ChildCompletion {
            child_thread_id,
            parent_id,
            parent_is_coding_agent,
            label: child_label(title, first_message),
            status: ChildCompletionStatus::Canceled,
            summary: settled_by.summary().to_string(),
            terminal_event_id: None,
        }))
    }

    /// Deliver a card [`Self::owed_child_card`] decided is owed.
    pub(crate) async fn deliver_owed_child_card(&self, card: OwedChildCard) {
        crate::log!(
            "[FanOut] Child {} settled by the user: sending its parent {} the canceled card",
            card.0.child_thread_id,
            card.0.parent_id
        );
        self.deliver_child_completion(card.0).await;
    }

    /// Wake the parent and deliver one `ChildThreadCompleted` to it.
    async fn deliver_child_completion(&self, card: ChildCompletion) {
        let ChildCompletion {
            child_thread_id,
            parent_id,
            parent_is_coding_agent,
            label,
            status,
            summary,
            terminal_event_id,
        } = card;
        // Held for the rest of the function: every path below that gives up on
        // driving a turn hands it to `undo_parent_wake`, so the parent is not
        // left reading "Requesting" against nothing.
        let wake = self.update_parent_after_child_terminal(parent_id).await;

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

        // Changes held below the child, whatever its own status: they are other
        // threads' work, and an orchestrator's children hold all of it.
        let sub_thread_pending_changes =
            match crate::core::changes::sub_thread_pending_changes(&self.pool, child_thread_id)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    crate::log!(
                        "[FanOut] sub_thread_pending_changes({}): {}; the card lists none",
                        child_thread_id,
                        e
                    );
                    Vec::new()
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
            child_thread_title: Some(label),
            status,
            summary,
            pending_change_ids,
            sub_thread_pending_changes,
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
                Self::undo_parent_wake(&self.pool, parent_id, wake.as_ref()).await;
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
                Self::undo_parent_wake(&self.pool, parent_id, wake.as_ref()).await;
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
            Self::undo_parent_wake(&self.pool, parent_id, wake.as_ref()).await;
            return;
        };
        // The wake promised a turn. If the channel is gone (its receiver dies
        // only as the engine goes down) nothing will drive one, so put the
        // status back. The card stays: `refire_unprocessed_child_completions`
        // re-drives this wake on the next boot, and a rollback that writes no
        // event leaves that sweep's selection intact.
        if !self.send_parent_callback(
            parent_id,
            child_thread_id,
            emit_result.event_id,
            terminal_event_id,
            parent_is_cc,
        ) {
            Self::undo_parent_wake(&self.pool, parent_id, wake.as_ref()).await;
        }
    }

    /// Announce a terminal the fan-in withheld, because the auto-resume it was
    /// withheld for is not going to happen.
    ///
    /// The withheld terminal was always a `ResponseFailed`: only
    /// `is_transient_api_failure` takes a hold. So the replay reconstructs one
    /// from the error the hold carried, and the parent gets the same
    /// `status: failure` card the un-held path would have sent.
    ///
    /// No terminal event id, because this is a replay rather than an emit. The
    /// stand-down gate abstains without one and may cost a duplicate turn, which
    /// is the recoverable direction. `refire_unprocessed_child_completions`
    /// passes `None` for the same reason.
    pub(crate) async fn announce_withheld_completion(&self, child_thread_id: Uuid, error: String) {
        crate::log!(
            "[FanOut] Child {} was not resumed after all: announcing the completion card its \
             terminal held back",
            child_thread_id
        );
        Box::pin(self.notify_parent_if_child(
            child_thread_id,
            None,
            &ThreadEvent::ResponseFailed { error },
        ))
        .await;
    }

    /// Mark the parent awake on a child completion that warrants user
    /// attention (`should_callback`): inbox, `running`, then one aggregate
    /// broadcast carrying both plus any counter change from the in-tx
    /// reconcile.
    ///
    /// **`running` is what makes the wake visible.** A user message flips the
    /// thread the instant it persists, so its exchange opens and reads
    /// "Requesting" through the turn setup. Leaving the parent `idle` until
    /// the turn's first activity event reads as a thread that never woke.
    ///
    /// The flip belongs here, not in the `ChildThreadCompleted` projection
    /// arm, because this runs under the gate that decides a card is owed. So
    /// `status_transitions()` still omits the event: that table mirrors
    /// `update_thread_projection`, and this write is not in it.
    ///
    /// **`waiting_for_user_answer` is preserved**, the one status a wake must
    /// not touch. `is_attention_needing` returns TRUE on it before any other
    /// test, so overwriting it darkens the badge on a parent whose question is
    /// still unanswered. `main.rs`'s boot reset scopes itself the same way.
    ///
    /// `active_children_count` is NOT written here. The in-tx
    /// `reconcile_parent_active_children_count` recomputes it from ground
    /// truth and is its sole writer; a second decrement here double-counts.
    ///
    /// Returns what it changed, so a caller that then fails to drive a turn
    /// can put it back. `None` means the parent's row is gone, so the UPDATE
    /// matched nothing and there is nothing to undo. Rationale and invariants:
    /// `docs/plans/2026-08-19-waking-a-parent-starts-its-exchange.md`.
    pub(super) async fn update_parent_after_child_terminal(
        &self,
        parent_id: Uuid,
    ) -> Option<ParentWake> {
        // The self-join reads the pre-UPDATE row in the same statement, so the
        // rollback value cannot be a second query racing the write.
        //
        // `FOR UPDATE` is load-bearing, not decoration. Two children of one
        // parent can finish at once. Under READ COMMITTED the second statement
        // blocks on the row lock at the UPDATE, while its `prev` scan keeps the
        // older snapshot. It would capture `idle` instead of the `running` the
        // first wake committed. A rollback would then settle a parent whose
        // other callback is still in flight. Locking in the scan makes the
        // capture wait for the same serialization the write does.
        type WakeRow = (
            i64,
            i64,
            String,
            Option<chrono::DateTime<Utc>>,
            chrono::DateTime<Utc>,
        );
        let row: Option<WakeRow> = match sqlx::query_as(
            "UPDATE thread_summaries t \
             SET archive_state = $2, \
                 status = CASE WHEN t.status = 'waiting_for_user_answer' \
                               THEN t.status ELSE 'running' END, \
                 last_revived_at = NOW() \
             FROM (SELECT thread_id, status, last_revived_at \
                   FROM thread_summaries WHERE thread_id = $1 FOR UPDATE) prev \
             WHERE t.thread_id = prev.thread_id \
             RETURNING t.active_children_count::bigint, t.total_children_count::bigint, \
                       prev.status, prev.last_revived_at, t.last_revived_at",
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
                return None;
            }
        };
        // `None` here is the missing-parent-row case: nothing was written, so
        // there is no wake to report and nothing for a caller to undo.
        let (active, total, prev_status, prev_revived_at, revived_at) = row?;
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
        Some(ParentWake {
            prev_status,
            prev_revived_at,
            revived_at,
        })
    }

    /// Roll the wake back on a path that gave up before any turn was driven.
    /// Left alone, the parent spins "Requesting" against nothing until the
    /// next boot reset.
    ///
    /// **A plain UPDATE, never `settle_stuck_running_thread`.**
    /// `refire_unprocessed_child_completions` selects on the card being the
    /// thread's LAST event. A terminal emitted after it would take the parent
    /// out of that sweep and strand the wake for good. Writing no event keeps
    /// the retry intact, which is what makes this safe on the shutdown path,
    /// where the card DID persist.
    ///
    /// Restores the previous status rather than assuming `idle`: a parent
    /// already mid-turn when its child finished must not be settled by a
    /// rollback. `last_revived_at` is the fencing token for the same reason,
    /// and goes back with it, so two wakes unwind in any order. See
    /// `ParentWake`.
    ///
    /// Takes the pool rather than `&self`, for the reason
    /// `authorize_child_follow_up` does: the whole thing is one guarded query,
    /// so it is directly testable without standing up a bus.
    pub(super) async fn undo_parent_wake(
        pool: &sqlx::PgPool,
        parent_id: Uuid,
        wake: Option<&ParentWake>,
    ) {
        // No wake to undo: `should_callback` was false, the parent's row is
        // gone, or the write itself failed.
        let Some(wake) = wake else { return };
        if let Err(e) = sqlx::query(
            "UPDATE thread_summaries SET status = $2, last_revived_at = $3 \
             WHERE thread_id = $1 AND last_revived_at = $4",
        )
        .bind(parent_id)
        .bind(&wake.prev_status)
        .bind(wake.prev_revived_at)
        .bind(wake.revived_at)
        .execute(pool)
        .await
        {
            crate::log!(
                "[FanOut] Failed to undo the wake for parent {} after a lost card: {}",
                parent_id,
                e
            );
        }
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
        // a real fan-in. See the selection rationale above. A later
        // `ChildThreadStopped` or `ChildThreadDetached` is not a reaction: it
        // wakes nothing, so a card it follows is still unprocessed (ADR 0252,
        // ADR 0278). No card was written after its child moved out, since the
        // emit refuses one, so every card this finds was earned.
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
                   AND later.event_type NOT IN ('ChildThreadStopped', 'ChildThreadDetached') \
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
            // Wake the parent here too, exactly as the live fan-in does. A
            // re-fired wake runs the same turn with the same setup cost, so
            // without this the recovered parent reads "Done" for that whole
            // window: the very gap this path is re-firing to close.
            let wake = self.update_parent_after_child_terminal(parent_id).await;
            // No terminal id: this sweep reads the completion card, and the
            // child terminal behind it is not recorded on the row. The gate
            // abstains, which is the fail-open side.
            if !self.send_parent_callback(
                parent_id,
                child_id,
                event_id,
                None,
                parent_is_coding_agent,
            ) {
                Self::undo_parent_wake(&self.pool, parent_id, wake.as_ref()).await;
                continue;
            }
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

    /// Hand the wake to the listener task. Returns whether it was accepted, so
    /// a caller that already marked the parent awake can roll that back when
    /// no turn will follow.
    fn send_parent_callback(
        &self,
        parent_thread_id: Uuid,
        child_thread_id: Uuid,
        child_completed_event_id: Uuid,
        child_terminal_event_id: Option<Uuid>,
        parent_is_coding_agent: bool,
    ) -> bool {
        if let Err(e) = self.parent_callback_tx.send(ParentCallback {
            parent_thread_id,
            child_thread_id,
            child_completed_event_id,
            child_terminal_event_id,
            parent_is_coding_agent,
        }) {
            crate::log!(
                "[FanOut] Failed to send parent callback for child {}: {}",
                child_thread_id,
                e
            );
            return false;
        }
        true
    }
}
