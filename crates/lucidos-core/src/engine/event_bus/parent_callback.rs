use chrono::Utc;
use uuid::Uuid;

use super::{turn_ended_as_redirect_sql, BusEvent, EmittedEvent, EventBus, ParentCallback};
use crate::engine::continuation_reason::ENGINE_RESTART_INTERRUPT_REASON;
use crate::engine::thread_events::{
    CancelCause, ChildCompletionStatus, EventMeta, EventWaitCancelCause, ProviderError,
    ThreadEvent, WakeMode,
};
use crate::engine::thread_lifecycle::{ArchiveState, ChangeStateKind, ThreadStatus};

#[derive(sqlx::FromRow)]
struct ChildRow {
    parent_thread_id: Option<Uuid>,
    is_coding_agent: bool,
    title: Option<String>,
    first_message: Option<String>,
    parent_callback_pending: bool,
    is_stopped_child: bool,
    holds_live_wait: bool,
    status: ThreadStatus,
    coding_agent_change_state: ChangeStateKind,
    parent_is_coding_agent: Option<bool>,
    parent_wake_requested: Option<bool>,
    wake_mode: Option<String>,
    batch_still_working: bool,
    keep_alive_turn: bool,
    handed_off: bool,
}

impl ChildRow {
    fn holds_unproposed_work(&self) -> bool {
        self.is_coding_agent && self.coding_agent_change_state == ChangeStateKind::Unproposed
    }

    fn wake_mode(&self) -> Option<WakeMode> {
        self.wake_mode.as_deref().and_then(WakeMode::parse)
    }
}

fn child_row_sql() -> String {
    format!(
        "SELECT c.parent_thread_id, c.is_coding_agent, c.title, \
            c.first_message, c.parent_callback_pending, c.is_stopped_child, \
            c.live_event_wait_count > 0 AS holds_live_wait, c.status, \
            c.coding_agent_change_state, c.parent_wake_requested, c.wake_mode, \
            c.keep_alive_turn, \
            (c.successor_thread_id IS NOT NULL OR {}) AS handed_off, \
            p.is_coding_agent AS parent_is_coding_agent, \
            {} AS batch_still_working \
     FROM thread_summaries c \
     LEFT JOIN thread_summaries p ON p.thread_id = c.parent_thread_id \
     WHERE c.thread_id = $1",
        crate::engine::handoff_sql::pending_handoff_sql("c.thread_id"),
        batch_still_working_sql("c", "$2", "$3")
    )
}

/// INVARIANT: SQL for "another child of `child`'s parent, spawned `wake-when-all-done`,
/// is still working": the batch [`Delivery::for_report`] waits on. Bind
/// `active_thread_statuses` to `statuses_param` and the batch mode's name to
/// `mode_param`. The fan-in and the turn-end gate both read it, so the read
/// decision's forecast matches the delivery. A sibling whose handoff is
/// pending works on: its successor session is about to start. A sibling that
/// handed off to a legacy successor thread works on until that thread's row exists,
/// since the Thread Queue may hold the spawn. From then on the successor counts
/// as the sibling.
pub fn batch_still_working_sql(child: &str, statuses_param: &str, mode_param: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM thread_summaries s \
                  WHERE s.parent_thread_id = {child}.parent_thread_id \
                    AND s.thread_id <> {child}.thread_id AND s.wake_mode = {mode_param} \
                    AND CASE WHEN s.successor_thread_id IS NULL THEN ({} OR {}) \
                        ELSE NOT EXISTS (SELECT 1 FROM thread_summaries n \
                                         WHERE n.thread_id = s.successor_thread_id) END)",
        crate::core::store::still_working_sql("s", statuses_param),
        crate::engine::handoff_sql::pending_handoff_sql("s.thread_id")
    )
}

const SUCCESS_SUMMARY_CAP: usize = 2000;
const FAILURE_SUMMARY_CAP: usize = 200;

const INTERRUPTED_SUMMARY: &str = "An engine restart cut this child's turn before it \
     finished, and nothing will resume it on its own. Its work so far is kept. To continue, \
     send it a follow-up with follow_up_child_thread. If its own work may have \
     crashed the engine, such as a heavy build or test run, ask the user before restarting it.";

const BATCH_ABORT_SUMMARY: &str = "This child's turn was aborted before it finished, \
     with no report of its own, and nothing will resume it. It was the last child of your \
     wake-when-all-done batch still working, so this card wakes you to combine what the \
     batch filed. Send it a follow-up with follow_up_child_thread if you need its part.";

fn restart_cut_without_resume(
    event: &ThreadEvent,
    actor: Option<&crate::engine::thread_events::MessageOrigin>,
) -> bool {
    matches!(
        event,
        ThreadEvent::ResponseAborted { cause, .. }
            if cause.is_transient() && !cause.promises_auto_resume(actor)
    )
}

fn cap_summary(summary: String, cap: usize) -> String {
    if summary.len() <= cap {
        return summary;
    }
    let cut = summary.floor_char_boundary(cap);
    format!("{}… (truncated)", &summary[..cut])
}

/// INVARIANT: The failure card's text. A provider policy block keeps the provider's own
/// words and adds what the parent must do next. Its tail never falls to the
/// cut, so the provider's words give way to fit `cap`. Every other failure
/// relays `error` as it is.
fn failure_summary(error: &str, provider_error: Option<&ProviderError>, cap: usize) -> String {
    let Some(policy) = provider_error.and_then(ProviderError::policy_name) else {
        return cap_summary(error.to_string(), cap);
    };
    let explanation = format!(
        "OpenAI blocked this turn: {policy}. The next message starts a new session. \
         Rephrase the order or ask the user, never resend it."
    );
    let budget = cap.saturating_sub(explanation.len() + 1);
    let words = &error[..error.floor_char_boundary(budget)];
    format!("{words} {explanation}")
}

fn in_flight(status: ThreadStatus) -> bool {
    crate::core::store::active_thread_statuses().contains(&status.as_str())
}

fn child_label(title: Option<String>, first_message: Option<String>) -> String {
    title
        .or_else(|| first_message.map(|m| m.chars().take(80).collect()))
        .unwrap_or_else(|| "unknown task".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildSettle {
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

    fn ends_waits(self) -> bool {
        !matches!(self, Self::Discarded)
    }
}

/// INVARIANT: A canceled card a settle decided is owed, read while the child's row still
/// exists. Delivered separately, because a delete must read before it deletes
/// and deliver after it commits.
pub struct OwedChildCard(ChildCompletion);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Wake,
    File,
}

impl Delivery {
    pub fn for_report(
        status: ChildCompletionStatus,
        wake_mode: Option<WakeMode>,
        parent_wake_requested: Option<bool>,
        batch_still_working: bool,
    ) -> Self {
        let clean = matches!(
            status,
            ChildCompletionStatus::Success | ChildCompletionStatus::NoChanges
        );
        let woken = !clean || parent_wake_requested == Some(true);
        match wake_mode {
            Some(WakeMode::NoWake) => Self::File,
            _ if woken => Self::Wake,
            Some(WakeMode::Wake) => Self::Wake,
            Some(WakeMode::WakeWhenAllDone) if batch_still_working => Self::File,
            Some(WakeMode::WakeWhenAllDone) => Self::Wake,
            None if parent_wake_requested == Some(false) => Self::File,
            None => Self::Wake,
        }
    }

    fn for_owed_card(wake_mode: Option<WakeMode>) -> Self {
        if wake_mode == Some(WakeMode::NoWake) {
            Self::File
        } else {
            Self::Wake
        }
    }
}

struct ChildCompletion {
    child_thread_id: Uuid,
    parent_id: Uuid,
    parent_is_coding_agent: Option<bool>,
    label: String,
    status: ChildCompletionStatus,
    summary: String,
    terminal_event_id: Option<Uuid>,
    delivery: Delivery,
}

/// INVARIANT: What a wake changed on the parent's row, so a caller that then fails to
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
    /// INVARIANT: Nullable: a thread that has never been revived carries no stamp.
    pub(super) prev_revived_at: Option<chrono::DateTime<Utc>>,
    pub(super) revived_at: chrono::DateTime<Utc>,
}

impl EventBus {
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
            emitting_trigger_id: crate::engine::trigger_context::current_trigger_id(),
        });
    }

    pub(super) async fn broadcast_children_count(&self, parent_id: Uuid) {
        let counts: Option<(i64, i64)> = match sqlx::query_as(
            "SELECT active_children_count::bigint, total_children_count::bigint FROM thread_summaries WHERE thread_id = $1"
        )
        .bind(parent_id)
        .fetch_optional(&self.pool)
        .await {
            Ok(row) => row,
            Err(e) => {
                crate::log_legacy!("[EventBus] Failed to query children counts for {}: {}", parent_id, e);
                return;
            }
        };
        if let Some((active, total)) = counts {
            self.send_children_count_event(parent_id, active, total, None);
        }
    }

    pub(super) async fn notify_parent_if_child(
        &self,
        child_thread_id: Uuid,
        terminal_event_id: Option<Uuid>,
        event: &ThreadEvent,
        actor: Option<&crate::engine::thread_events::MessageOrigin>,
    ) {
        if let ThreadEvent::EventWaitCanceled { cause, .. } = event {
            self.announce_when_last_wait_ends(child_thread_id, *cause)
                .await;
            return;
        }

        // INVARIANT: Cancel = user-driven, terminal. Abort splits three ways:
        // - The user's own switch promises a resume: no decrement, no
        //   callback, since the resumed turn reports.
        // - A restart nobody promised to undo is a completion, reported as
        //   `interrupted`. A coding-agent child's recovery idle may follow,
        //   and the pending-card guard below swallows it.
        // - SafetyNet / ProcessKilled / Unknown are terminal (decrement so
        //   the parent doesn't display as Active forever, but no card: the
        //   user already sees the child's error state).
        //
        // Cancel splits on `CancelCause` for the same reason. A
        // `SupersededByFollowup` is the mid-turn redirect `arm_followup_redirect`
        // arms when a follow-up lands on a live Codex turn: the caller steered,
        // they did not abandon (`thread_events/cause.rs`), and the child runs
        // the redirected turn immediately after. An older `EffortRestart` is the
        // same shape: the engine resumed the child. Reporting it would wake the
        // parent with a false "child canceled" card for work that is still
        // running, and the parent may spawn a replacement. The redirected
        // turn's own terminal is the real report. Dropping out of the terminal
        // set here leaves the in-tx `reconcile_parent_active_children_count`
        // (which runs in the `ResponseCanceled` projection arm, cause-agnostic)
        // as the only thing that fires, so no counter drifts.
        let cut_without_resume = restart_cut_without_resume(event, actor);
        let is_terminal = match event {
            ThreadEvent::CodingAgentIdled { .. }
            | ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseFailed { .. } => true,
            ThreadEvent::ResponseCanceled { cause, .. } => !cause.is_redirect(),
            ThreadEvent::ResponseAborted { cause, .. } => !cause.promises_auto_resume(actor),
            ThreadEvent::SessionEnded { reason } => !reason.is_transient(),
            _ => false,
        };
        if !is_terminal {
            return;
        }

        if matches!(
            event,
            ThreadEvent::ResponseAborted {
                cause: crate::engine::thread_events::AbortCause::SnapshotRestored,
                ..
            }
        ) {
            return;
        }
        match crate::core::snapshot::boundary::thread_turn_after_restore(
            &self.pool,
            child_thread_id,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                crate::log_legacy!(
                    "[FanOut] Restore boundary unavailable; refusing parent callback: {}",
                    error
                );
                return;
            }
        }

        // INVARIANT: **A terminal the engine is about to auto-resume is not a completion.**
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
            crate::log_legacy!(
                "[FanOut] Child {} ended on a terminal the engine is auto-resuming: \
                 withholding the parent's completion card until the real terminal",
                child_thread_id
            );
            return;
        }

        let Some(row) = self.load_child_row(child_thread_id).await else {
            return;
        };
        if row.keep_alive_turn || row.handed_off {
            return;
        }
        let wake_mode = row.wake_mode();
        let ChildRow {
            parent_thread_id: Some(parent_id),
            is_coding_agent,
            title,
            first_message: first_msg,
            parent_callback_pending,
            is_stopped_child,
            holds_live_wait,
            parent_is_coding_agent,
            parent_wake_requested,
            batch_still_working,
            ..
        } = row
        else {
            return;
        };

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

        if holds_live_wait
            && matches!(
                event,
                ThreadEvent::CodingAgentIdled { .. }
                    | ThreadEvent::ResponseGenerated { .. }
                    | ThreadEvent::SessionEnded { .. }
            )
        {
            crate::log_legacy!(
                "[FanOut] Child {} ended a turn holding an event wait: no card until the \
                 turn the wait wakes ends",
                child_thread_id
            );
            return;
        }

        // INVARIANT: **The idle closing a redirected turn is no completion either.** Its
        // cancel stood down at `is_terminal`, and the turn the redirect runs
        // next reports. The marker stays set for it.
        if matches!(event, ThreadEvent::CodingAgentIdled { .. })
            && self.turn_ended_as_redirect(child_thread_id).await
        {
            crate::log_legacy!(
                "[FanOut] Child {} idled after a redirect: no card until the redirected turn ends",
                child_thread_id
            );
            return;
        }

        // INVARIANT: CC threads can emit CodingAgentIdled multiple times (initial work,
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

        // INVARIANT: Coding-agent sessions can terminate without ever emitting CodingAgentIdled or
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
        // and the parent pulses as "waiting for children" forever. A switch
        // abort, which promises a resume, already early-returned via
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
        let should_callback = matches!(
            (is_coding_agent, event),
            (true, ThreadEvent::CodingAgentIdled { .. })
                | (false, ThreadEvent::ResponseGenerated { .. })
                | (_, ThreadEvent::ResponseFailed { .. })
                | (_, ThreadEvent::ResponseCanceled { .. })
        ) || (cut_without_resume
            && (parent_callback_pending || !is_coding_agent))
            || (is_coding_agent
                && parent_callback_pending
                && matches!(event, ThreadEvent::SessionEnded { .. }));

        let clear_callback_for_terminal_abort = should_decrement
            && is_coding_agent
            && matches!(event, ThreadEvent::ResponseAborted { .. });

        if clear_callback_for_terminal_abort {
            self.clear_pending_parent_callback(child_thread_id).await;
        }

        if !should_callback {
            if should_decrement
                && matches!(event, ThreadEvent::ResponseAborted { .. })
                && wake_mode == Some(WakeMode::WakeWhenAllDone)
                && !batch_still_working
            {
                self.deliver_child_completion(
                    ChildCompletion {
                        child_thread_id,
                        parent_id,
                        parent_is_coding_agent,
                        label: child_label(title, first_msg),
                        status: ChildCompletionStatus::Failure,
                        summary: BATCH_ABORT_SUMMARY.to_string(),
                        terminal_event_id,
                        delivery: Delivery::Wake,
                    },
                    None,
                )
                .await;
            }
            return;
        }

        let label = child_label(title, first_msg);

        let last_response = self.last_response_text(child_thread_id).await;

        let (status, summary, cap) = match event {
            ThreadEvent::CodingAgentIdled {
                reason: Some(reason),
                ..
            } if reason == ENGINE_RESTART_INTERRUPT_REASON => (
                ChildCompletionStatus::Interrupted,
                INTERRUPTED_SUMMARY.to_string(),
                SUCCESS_SUMMARY_CAP,
            ),
            ThreadEvent::ResponseAborted { .. } => (
                ChildCompletionStatus::Interrupted,
                INTERRUPTED_SUMMARY.to_string(),
                SUCCESS_SUMMARY_CAP,
            ),
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
            ThreadEvent::ResponseFailed {
                error,
                provider_error,
                ..
            } => (
                ChildCompletionStatus::Failure,
                failure_summary(error, provider_error.as_ref(), FAILURE_SUMMARY_CAP),
                FAILURE_SUMMARY_CAP,
            ),
            // INVARIANT: A cancel that ended the child. Pull the
            // partial-stream text from ResponseCanceled itself; falling back
            // to last_response would surface a *prior* turn's text and read
            // as if the canceled turn had completed.
            ThreadEvent::ResponseCanceled { text, .. } => (
                ChildCompletionStatus::Canceled,
                text.clone(),
                SUCCESS_SUMMARY_CAP,
            ),
            _ => (
                ChildCompletionStatus::Success,
                last_response.clone().unwrap_or_default(),
                SUCCESS_SUMMARY_CAP,
            ),
        };

        let repeat_head = match event {
            ThreadEvent::CodingAgentIdled {
                worktree_head_sha, ..
            } if status == ChildCompletionStatus::Success
                && !summary.trim().is_empty()
                && summary.len() <= cap
                && parent_wake_requested != Some(true)
                && wake_mode != Some(WakeMode::WakeWhenAllDone) =>
            {
                worktree_head_sha.as_deref()
            }
            _ => None,
        };
        self.deliver_child_completion(
            ChildCompletion {
                child_thread_id,
                parent_id,
                parent_is_coding_agent,
                label,
                status,
                summary: cap_summary(summary, cap),
                terminal_event_id,
                delivery: Delivery::for_report(
                    status,
                    wake_mode,
                    parent_wake_requested,
                    batch_still_working,
                ),
            },
            repeat_head,
        )
        .await;
    }

    async fn load_child_row(&self, child_thread_id: Uuid) -> Option<ChildRow> {
        sqlx::query_as::<_, ChildRow>(&child_row_sql())
            .bind(child_thread_id)
            .bind(crate::core::store::active_thread_statuses())
            .bind(WakeMode::WakeWhenAllDone.as_str())
            .fetch_optional(&self.pool)
            .await
            .unwrap_or_else(|e| {
                crate::log_legacy!(
                    "[FanOut] Failed to look up child {}: {}",
                    child_thread_id,
                    e
                );
                None
            })
    }

    async fn turn_ended_as_redirect(&self, child_thread_id: Uuid) -> bool {
        sqlx::query_scalar::<_, bool>(&format!("SELECT {}", turn_ended_as_redirect_sql("$1")))
            .bind(child_thread_id.to_string())
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|e| {
                crate::log_legacy!(
                    "[FanOut] Failed to read child {}'s last terminal: {}",
                    child_thread_id,
                    e
                );
                false
            })
    }

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
            crate::log_legacy!(
                "[FanOut] Failed to fetch child response for {}: {}",
                child_thread_id,
                e
            );
            None
        })
    }

    async fn announce_when_last_wait_ends(
        &self,
        child_thread_id: Uuid,
        cause: EventWaitCancelCause,
    ) {
        if cause.leaves_thread_open() {
            self.send_held_card(child_thread_id, true).await;
        }
    }

    /// INVARIANT: Whether `child_thread_id`'s held card waits on the branch-work net to
    /// propose its work first. Read it BEFORE proposing: the proposal is what
    /// clears it.
    pub async fn held_card_awaits_proposal(&self, child_thread_id: Uuid) -> bool {
        self.load_child_row(child_thread_id)
            .await
            .is_some_and(|row| row.parent_callback_pending && row.holds_unproposed_work())
    }

    pub async fn send_held_card_after_proposal(&self, child_thread_id: Uuid) {
        self.send_held_card(child_thread_id, false).await;
    }

    async fn send_held_card(&self, child_thread_id: Uuid, defer_unproposed_work: bool) {
        let Some(row) = self.load_child_row(child_thread_id).await else {
            return;
        };
        if defer_unproposed_work && row.holds_unproposed_work() {
            return;
        }
        let delivery = Delivery::for_owed_card(row.wake_mode());
        let ChildRow {
            parent_thread_id: Some(parent_id),
            is_coding_agent,
            title,
            first_message: first_msg,
            parent_callback_pending: true,
            is_stopped_child: false,
            holds_live_wait: false,
            status,
            coding_agent_change_state,
            parent_is_coding_agent,
            ..
        } = row
        else {
            return;
        };
        if status != ThreadStatus::Idle {
            return;
        }
        crate::log_legacy!(
            "[FanOut] Child {}'s last event wait ended without waking it: sending its \
             parent {} the card it held back",
            child_thread_id,
            parent_id
        );
        let summary = self
            .last_response_text(child_thread_id)
            .await
            .unwrap_or_default();
        self.deliver_child_completion(
            ChildCompletion {
                child_thread_id,
                parent_id,
                parent_is_coding_agent,
                label: child_label(title, first_msg),
                status: if is_coding_agent && coding_agent_change_state == ChangeStateKind::None {
                    ChildCompletionStatus::NoChanges
                } else {
                    ChildCompletionStatus::Success
                },
                summary: cap_summary(summary, SUCCESS_SUMMARY_CAP),
                terminal_event_id: None,
                delivery,
            },
            None,
        )
        .await;
    }

    async fn announce_stopped_child(&self, parent_id: Uuid, child_thread_id: Uuid, label: String) {
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

    pub async fn settle_child(&self, child_thread_id: Uuid, settled_by: ChildSettle) {
        if let Some(card) = self.owed_child_card(child_thread_id, settled_by).await {
            self.deliver_owed_child_card(card).await;
        }
    }

    pub async fn owed_child_card(
        &self,
        child_thread_id: Uuid,
        settled_by: ChildSettle,
    ) -> Option<OwedChildCard> {
        let row = self.load_child_row(child_thread_id).await?;
        let delivery = Delivery::for_owed_card(row.wake_mode());
        let ChildRow {
            parent_thread_id: Some(parent_id),
            title,
            first_message,
            parent_callback_pending: true,
            holds_live_wait,
            status,
            parent_is_coding_agent,
            ..
        } = row
        else {
            return None;
        };
        if in_flight(status) || (holds_live_wait && !settled_by.ends_waits()) {
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
            delivery,
        }))
    }

    pub async fn deliver_owed_child_card(&self, card: OwedChildCard) {
        crate::log_legacy!(
            "[FanOut] Child {} settled by the user: sending its parent {} the canceled card",
            card.0.child_thread_id,
            card.0.parent_id
        );
        self.deliver_child_completion(card.0, None).await;
    }

    async fn deliver_child_completion(&self, card: ChildCompletion, repeat_head: Option<&str>) {
        let ChildCompletion {
            child_thread_id,
            parent_id,
            parent_is_coding_agent,
            label,
            status,
            summary,
            terminal_event_id,
            delivery,
        } = card;
        let (pending_change_ids, sub_thread_pending_changes, changes_read) =
            self.reported_changes(child_thread_id, status).await;
        let integrated_branch = self
            .reported_integrated_branch(child_thread_id, status)
            .await;
        let filed_event = || ThreadEvent::ChildReportFiled {
            child_thread_id,
            child_thread_title: Some(label.clone()),
            status,
            summary: summary.clone(),
            pending_change_ids: pending_change_ids.clone(),
            sub_thread_pending_changes: sub_thread_pending_changes.clone(),
            integrated_branch: integrated_branch.clone(),
        };
        let repeat = if delivery == Delivery::Wake
            && changes_read
            && integrated_branch
                .as_ref()
                .is_some_and(|branch| repeat_head == Some(branch.head_sha.as_str()))
        {
            let report = filed_event();
            self.matches_last_child_report(parent_id, child_thread_id, terminal_event_id, &report)
                .await
                .then_some(report)
        } else {
            None
        };
        let held_parent = repeat.is_none()
            && delivery == Delivery::Wake
            && if parent_is_coding_agent == Some(true) {
                !crate::core::store::thread_hold::may_start_turn(&self.pool, parent_id).await
            } else {
                crate::core::store::thread_stop::thread_is_stopped(&self.pool, parent_id).await
                    || crate::core::store::thread_hold::thread_is_held(&self.pool, parent_id).await
            };
        if let Some(report) =
            repeat.or_else(|| (delivery == Delivery::File || held_parent).then(filed_event))
        {
            self.file_child_report(parent_id, child_thread_id, report)
                .await;
            return;
        }
        // INVARIANT: Held for the rest of the function: every path below that gives up on
        // driving a turn hands it to `undo_parent_wake`, so the parent is not
        // left reading "Requesting" against nothing.
        let wake = self.update_parent_after_child_terminal(parent_id).await;

        // INVARIANT: Emit the typed source-of-truth event onto the parent thread. The
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
            integrated_branch,
        };
        // INVARIANT: Box::pin here because `emit_or_log → emit → notify_parent_if_child`
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
                crate::log_legacy!(
                    "[FanOut] ChildThreadCompleted emit returned None for parent {}; \
                     skipping wake-up kick — see notify_parent_if_child error path.",
                    parent_id
                );
                self.roll_back_parent_wake(parent_id, wake.as_ref()).await;
                return;
            }
            Err(e) => {
                crate::log_legacy!(
                    "[FanOut] Failed to emit ChildThreadCompleted for parent {}: {}; \
                     skipping wake-up kick — driving the parent against a non-persisted \
                     event would attribute its response to a phantom card. Next terminal \
                     event (if any) retries; for chat children with one-shot \
                     ResponseGenerated this means a permanent miss.",
                    parent_id,
                    e
                );
                self.roll_back_parent_wake(parent_id, wake.as_ref()).await;
                return;
            }
        };

        // INVARIANT: Defensive: `parent_is_coding_agent` should always be `Some` here
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
            crate::log_legacy!(
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
                crate::log_legacy!(
                    "[FanOut] Failed to re-mark the pending callback for child {}: {}",
                    child_thread_id,
                    e
                );
            }
            self.roll_back_parent_wake(parent_id, wake.as_ref()).await;
            return;
        };
        // INVARIANT: The wake promised a turn. If the channel is gone (its receiver dies
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
            self.roll_back_parent_wake(parent_id, wake.as_ref()).await;
        }
    }

    async fn file_child_report(&self, parent_id: Uuid, child_thread_id: Uuid, event: ThreadEvent) {
        crate::log_legacy!(
            "[FanOut] Child {}'s report does not wake its parent {}: filing it",
            child_thread_id,
            parent_id
        );
        Box::pin(self.emit_or_log(
            BusEvent::Thread {
                thread_id: parent_id,
                event,
                meta: EventMeta::NONE,
            },
            "[FanOut] ChildReportFiled",
        ))
        .await;
    }

    async fn matches_last_child_report(
        &self,
        parent_id: Uuid,
        child_thread_id: Uuid,
        idle_id: Option<Uuid>,
        report: &ThreadEvent,
    ) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT payload = $3 AND COALESCE($3->>'summary' = ( \
                 SELECT CASE WHEN event_type = 'ResponseGenerated' THEN payload->>'text' END \
                   FROM events WHERE aggregate = 'thread' AND aggregate_id = $2 \
                    AND event_type IN ('ResponseGenerated', 'CodingAgentIdled') \
                    AND sequence < (SELECT sequence FROM events WHERE id = $5) \
                  ORDER BY sequence DESC LIMIT 1), FALSE) \
               FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
                AND payload->>'child_thread_id' = $2 AND event_type = ANY($4) \
              ORDER BY sequence DESC LIMIT 1",
        )
        .bind(parent_id.to_string())
        .bind(child_thread_id.to_string())
        .bind(report.to_payload(&EventMeta::NONE))
        .bind(crate::core::store::messages::CHILD_REPORT_EVENT_TYPES)
        .bind(idle_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|error| {
            crate::log_legacy!(
                "[FanOut] Failed to read child {child_thread_id}'s last report: {error}"
            );
            None
        })
        .unwrap_or(false)
    }

    async fn reported_integrated_branch(
        &self,
        child_thread_id: Uuid,
        status: ChildCompletionStatus,
    ) -> Option<crate::engine::thread_events::IntegratedBranch> {
        if status != ChildCompletionStatus::Success {
            return None;
        }
        crate::core::store::integrated::handed_up_branch(&self.pool, child_thread_id).await
    }

    async fn reported_changes(
        &self,
        child_thread_id: Uuid,
        status: ChildCompletionStatus,
    ) -> (
        Vec<String>,
        Vec<crate::engine::thread_events::SubThreadPendingChange>,
        bool,
    ) {
        let mut complete = true;
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
                    complete = false;
                    crate::log_legacy!(
                        "[FanOut] pending_for_thread({}): {}; the report lists no \
                         pending_change_ids",
                        child_thread_id,
                        e
                    );
                    Vec::new()
                }
            }
        };

        let sub_thread_pending_changes =
            match crate::core::changes::sub_thread_pending_changes(&self.pool, child_thread_id)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    complete = false;
                    crate::log_legacy!(
                        "[FanOut] sub_thread_pending_changes({}): {}; the report lists none",
                        child_thread_id,
                        e
                    );
                    Vec::new()
                }
            };
        (pending_change_ids, sub_thread_pending_changes, complete)
    }

    /// INVARIANT: Announce a terminal the fan-in withheld, because the auto-resume it was
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
    pub async fn announce_withheld_completion(&self, child_thread_id: Uuid, error: String) {
        crate::log_legacy!(
            "[FanOut] Child {} was not resumed after all: announcing the completion card its \
             terminal held back",
            child_thread_id
        );
        Box::pin(self.notify_parent_if_child(
            child_thread_id,
            None,
            &ThreadEvent::ResponseFailed {
                error,
                spend_cap: None,
                agent_exit: None,
                provider_error: None,
            },
            None,
        ))
        .await;
    }

    /// INVARIANT: Mark the parent awake on a child completion that warrants user
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
        // INVARIANT: The self-join reads the pre-UPDATE row in the same statement, so the
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
        let row: Option<WakeRow> = match sqlx::query_as(&format!(
            "UPDATE thread_summaries t \
             SET archive_state = $2, \
                 status = CASE WHEN t.status = {waiting_for_user_answer} \
                               THEN t.status ELSE {running} END, \
                 last_revived_at = NOW() \
             FROM (SELECT thread_id, status, last_revived_at \
                   FROM thread_summaries WHERE thread_id = $1 FOR UPDATE) prev \
             WHERE t.thread_id = prev.thread_id \
             RETURNING t.active_children_count::bigint, t.total_children_count::bigint, \
                       prev.status, prev.last_revived_at, t.last_revived_at",
            waiting_for_user_answer = ThreadStatus::WaitingForUserAnswer.sql_literal(),
            running = ThreadStatus::Running.sql_literal(),
        ))
        .bind(parent_id)
        .bind(ArchiveState::Inbox.as_str())
        .fetch_optional(&self.pool)
        .await
        {
            Ok(opt) => opt,
            Err(e) => {
                crate::log_legacy!(
                    "[FanOut] Failed to update parent {} after child terminal: {}",
                    parent_id,
                    e
                );
                return None;
            }
        };
        let (active, total, prev_status, prev_revived_at, revived_at) = row?;
        self.request_archive_recount(parent_id);
        let aggregate =
            match crate::core::store::fetch_thread_aggregate(&self.pool, parent_id).await {
                Ok(agg) => agg,
                Err(e) => {
                    crate::log_legacy!(
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

    /// INVARIANT: Roll the wake back on a path that gave up before any turn was driven.
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
            crate::log_legacy!(
                "[FanOut] Failed to undo the wake for parent {} after a lost card: {}",
                parent_id,
                e
            );
        }
    }

    async fn roll_back_parent_wake(&self, parent_id: Uuid, wake: Option<&ParentWake>) {
        if wake.is_none() {
            return;
        }
        Self::undo_parent_wake(&self.pool, parent_id, wake).await;
        self.request_archive_recount(parent_id);
    }

    async fn clear_pending_parent_callback(&self, child_thread_id: Uuid) {
        if let Err(e) = sqlx::query(
            "UPDATE thread_summaries SET parent_callback_pending = FALSE WHERE thread_id = $1",
        )
        .bind(child_thread_id)
        .execute(&self.pool)
        .await
        {
            crate::log_legacy!(
                "[FanOut] Failed to clear the pending callback for child {}: {}",
                child_thread_id,
                e
            );
        }
    }

    pub async fn refire_unprocessed_child_completions(&self) -> usize {
        // INVARIANT: Restored completion cards are history on every boot, including
        // after the first-boot marker is gone. An unknown floor refuses wakes.
        let floor = match crate::core::snapshot::boundary::read(&self.pool).await {
            Ok(boundary) => boundary.map_or(0, |boundary| boundary.event_sequence),
            Err(error) => {
                crate::log_legacy!("[FanOut] restore boundary unavailable: {}", error);
                return 0;
            }
        };
        let rows: Vec<(Uuid, String, Option<String>, bool)> = match sqlx::query_as(
            "SELECT e.id, e.aggregate_id, e.payload->>'child_thread_id', p.is_coding_agent \
             FROM events e \
             JOIN thread_summaries p ON p.thread_id = e.aggregate_id::uuid \
             LEFT JOIN thread_summaries c \
               ON c.thread_id::text = e.payload->>'child_thread_id' \
             WHERE e.aggregate = 'thread' \
               AND e.event_type = 'ChildThreadCompleted' AND e.sequence > $4 \
               AND p.state IS DISTINCT FROM 'discarded' \
               AND c.wake_mode IS DISTINCT FROM $3 \
               AND NOT EXISTS ( \
                 SELECT 1 FROM events later \
                 WHERE later.aggregate = 'thread' \
                   AND later.aggregate_id = e.aggregate_id \
                   AND later.sequence > e.sequence \
                   AND later.event_type <> ALL($1) \
                   AND later.event_type <> ALL($2) \
               )",
        )
        .bind(crate::engine::thread_events::ThreadEvent::PARENT_NOTE_EVENT_TYPES)
        .bind(crate::engine::thread_events::ThreadEvent::SIDE_QUESTION_EVENT_TYPES)
        .bind(WakeMode::NoWake.as_str())
        .bind(floor)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                crate::log_legacy!(
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
                crate::log_legacy!(
                    "[FanOut] Skipping unprocessed ChildThreadCompleted {} — \
                     malformed parent ({:?}) or child ({:?}) id",
                    event_id,
                    parent_str,
                    child_str
                );
                continue;
            };
            crate::log_legacy!(
                "[FanOut] Re-firing lost parent wake: child {} completed, parent {} never resumed (event {})",
                child_id,
                parent_id,
                event_id
            );
            let wake = self.update_parent_after_child_terminal(parent_id).await;
            if !self.send_parent_callback(
                parent_id,
                child_id,
                event_id,
                None,
                parent_is_coding_agent,
            ) {
                self.roll_back_parent_wake(parent_id, wake.as_ref()).await;
                continue;
            }
            refired += 1;
        }
        if refired > 0 {
            crate::log_legacy!(
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
        child_terminal_event_id: Option<Uuid>,
        parent_is_coding_agent: bool,
    ) -> bool {
        let Some(callback) = self.parent_wake_hold.intercept(ParentCallback {
            parent_thread_id,
            child_thread_id,
            child_completed_event_id,
            child_terminal_event_id,
            parent_is_coding_agent,
        }) else {
            return true;
        };
        if let Err(e) = self.parent_callback_tx.send(callback) {
            crate::log_legacy!(
                "[FanOut] Failed to send parent callback for child {}: {}",
                child_thread_id,
                e
            );
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(code: &str) -> ProviderError {
        ProviderError {
            code: code.to_string(),
            detail: None,
        }
    }

    const OPENAI_WORDS: &str = "This request was blocked by our safety systems.";

    #[test]
    fn a_policy_block_keeps_the_providers_words_and_says_what_to_do() {
        for (code, policy) in [
            (
                "misalignment_policy_violation",
                "potentially unintended activity",
            ),
            ("cyber_policy", "cybersecurity policy"),
            ("bio_policy", "biosecurity policy"),
        ] {
            let summary = failure_summary(OPENAI_WORDS, Some(&block(code)), FAILURE_SUMMARY_CAP);
            assert!(summary.starts_with(OPENAI_WORDS), "{summary}");
            assert!(summary.contains(policy), "{summary}");
            assert!(summary.contains("new session"), "{summary}");
            assert!(summary.contains("never resend"), "{summary}");
            assert!(summary.len() <= FAILURE_SUMMARY_CAP, "{summary}");
        }
    }

    #[test]
    fn a_long_provider_text_gives_way_but_the_instruction_stays() {
        let long = "x".repeat(900);
        let summary = failure_summary(&long, Some(&block("cyber_policy")), FAILURE_SUMMARY_CAP);
        assert!(summary.ends_with("never resend it."), "{summary}");
        assert!(summary.len() <= FAILURE_SUMMARY_CAP, "{summary}");
    }

    #[test]
    fn any_other_provider_code_relays_the_error_unchanged() {
        for code in ["usage_limit_exceeded", "something_new"] {
            let summary = failure_summary("it broke", Some(&block(code)), FAILURE_SUMMARY_CAP);
            assert_eq!(summary, "it broke");
        }
        assert_eq!(
            failure_summary("it broke", None, FAILURE_SUMMARY_CAP),
            "it broke"
        );
    }
}
