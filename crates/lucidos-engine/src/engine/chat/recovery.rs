use std::sync::Arc;

use crate::core::EventRow;
use crate::engine::LucidosEngine;

/// SQL the orphan-recovery sweep runs to find chat / trigger / CC threads
/// whose last exchange has activity events but no terminal event. Extracted
/// from `recover_orphaned_threads` as a `const` so the test in the sibling
/// module can run the exact same query against a hand-built fixture and
/// assert the filter contract (archived threads are excluded) without
/// duplicating the SQL.
const ORPHAN_THREADS_SQL: &str = r#"
WITH candidate_threads AS (
    -- Skip archived threads outright. The user dismissed them;
    -- emitting a fresh `ResponseAborted` here would route the
    -- thread back to inbox via the contract layer's `to_inbox`
    -- rule (`thread_lifecycle::resolve_transition`), silently
    -- reviving a row the user deliberately closed. The
    -- projection's `status` column for those rows stays at
    -- whatever value it carried (typically `running`), but
    -- nothing user-visible reads `status` for archived threads
    -- — inbox / active queries already filter on
    -- `archive_state`.
    SELECT thread_id::text AS aggregate_id
    FROM thread_summaries
    WHERE thread_id != ALL($1::uuid[])
      AND archive_state != 'archived'
),
per_thread AS (
    SELECT
        e.aggregate_id,
        MAX(CASE WHEN e.event_type IN ('MessageReceived','TriggerStarted','ChildThreadCompleted')
                 THEN e.created END) AS last_start,
        MAX(CASE WHEN e.event_type IN ('TextStreamed','Thinking','ThoughtStreamed','ToolCalled','ToolResult',
                                        'CodingAgentTextStreamed','CodingAgentToolCalled','CodingAgentToolResult')
                 THEN e.created END) AS last_activity,
        MAX(CASE WHEN e.event_type IN ('ResponseGenerated','ResponseCanceled','ResponseAborted','ResponseFailed',
                                        'CodingAgentIdled','SessionEnded')
                 THEN e.created END) AS last_terminal
    FROM events e
    WHERE e.aggregate = 'thread'
      AND e.aggregate_id IN (SELECT aggregate_id FROM candidate_threads)
      AND e.event_type IN (
          'MessageReceived','TriggerStarted','ChildThreadCompleted',
          'TextStreamed','Thinking','ThoughtStreamed','ToolCalled','ToolResult',
          'CodingAgentTextStreamed','CodingAgentToolCalled','CodingAgentToolResult',
          'ResponseGenerated','ResponseCanceled','ResponseAborted','ResponseFailed',
          'CodingAgentIdled','SessionEnded'
      )
    GROUP BY e.aggregate_id
),
orphans AS (
    SELECT pt.aggregate_id::uuid AS thread_id, pt.last_start
    FROM per_thread pt
    WHERE pt.last_start IS NOT NULL
      AND pt.last_activity > pt.last_start
      AND (pt.last_terminal IS NULL OR pt.last_terminal <= pt.last_start)
)
SELECT o.thread_id,
       (SELECT LEFT(string_agg(e2.payload->>'text', '' ORDER BY e2.created), 2000)
        FROM events e2
        WHERE e2.aggregate_id = o.thread_id::text
          AND e2.event_type IN ('TextStreamed', 'CodingAgentTextStreamed')
          AND e2.created > o.last_start) AS partial_text,
       start_evt.id AS originating_event_id,
       start_evt.event_type AS originating_event_type,
       start_evt.channel AS originating_channel
FROM orphans o
LEFT JOIN LATERAL (
    SELECT e3.id, e3.event_type, e3.payload->>'channel' AS channel FROM events e3
    WHERE e3.aggregate_id = o.thread_id::text
      AND e3.event_type IN ('MessageReceived','TriggerStarted','ChildThreadCompleted')
      AND e3.created = o.last_start
    ORDER BY e3.sequence DESC LIMIT 1
) start_evt ON true
"#;

/// [`ORPHAN_THREADS_SQL`] plus the shared **preserve guard**: a thread parked on
/// an unanswered `AskUserQuestion` is a stable, resumable checkpoint, never an
/// interrupted turn — so it must NEVER be swept into a `ResponseAborted`. This
/// closes the reproduced gap where a coding-agent thread that
/// `recover_orphaned_worktrees` deliberately preserved (and therefore did NOT
/// add to `exclude_thread_ids`) was re-aborted here as "System — Response
/// interrupted", and the equivalent chat case (this sweep is the ONLY restart
/// abort path a question-parked chat thread hits). Same `unanswered_question_exists_sql`
/// fragment every other abort path uses, so the guard cannot drift.
fn orphan_threads_sql() -> String {
    format!(
        "{ORPHAN_THREADS_SQL}\nWHERE NOT {}",
        crate::engine::agent_recovery::unanswered_question_exists_sql("o.thread_id::text")
    )
}

/// [`ORPHAN_TOOL_CALLS_SQL`] plus the shared preserve guard, injected before the
/// `ORDER BY`. A question-parked chat thread has a dangling
/// `ToolCalled{ask_user_question}` (the loop emits it before blocking in
/// `walk_question_batch`); without this guard the sweep would synthesize a
/// "[Tool execution interrupted…]" `ToolResult` for it, poisoning the pending
/// question's tool-use pair so the resumed turn reads the answer as an error.
fn orphan_tool_calls_sql() -> String {
    ORPHAN_TOOL_CALLS_SQL.replace(
        "ORDER BY e.thread_id",
        &format!(
            "AND NOT {} ORDER BY e.thread_id",
            crate::engine::agent_recovery::unanswered_question_exists_sql("e.thread_id::text")
        ),
    )
}

/// SQL the orphan-recovery sweep runs to fetch the `(ToolCalled, ToolResult)`
/// pairs it pairs into orphans. Extracted from `recover_orphan_tool_calls`
/// for the same testability reason as `ORPHAN_THREADS_SQL`.
const ORPHAN_TOOL_CALLS_SQL: &str = r#"
SELECT e.id, e.event_type, e.payload, e.created, e.thread_id, e.sequence
FROM events e
JOIN thread_summaries ts ON ts.thread_id = e.thread_id
WHERE e.event_type IN ('ToolCalled', 'ToolResult')
  AND e.thread_id IS NOT NULL
  AND ts.archive_state != 'archived'
ORDER BY e.thread_id, e.created, e.sequence
"#;

impl LucidosEngine {
    /// Detect threads whose last exchange has activity events but no terminal event.
    /// These are in-flight threads (chat or CC) that died when the engine crashed.
    /// Handles both first-exchange orphans (has_response=false) and multi-exchange
    /// orphans where a previous exchange completed but the latest was interrupted
    /// (e.g., lid close during tool execution after a prior ResponseGenerated).
    ///
    /// Emits ResponseAborted via EventBus with partial text so the thread is
    /// marked UNREAD and the user notices. The actor is stamped as `System` so
    /// the AbortPanel renders "⚙ System" — the host system killed the previous
    /// response (engine crashed, OS killed the process, etc.); the engine on
    /// restart is just marking it. `request_event_id` is the originating
    /// MessageReceived/TriggerStarted id — the user-facing rerun path uses
    /// this to find the prompt to re-run.
    ///
    /// `exclude_thread_ids` — threads being actively recovered by worktree recovery;
    /// they should not be aborted here since CC recovery will handle them.
    pub async fn recover_orphaned_threads(self: &Arc<Self>, exclude_thread_ids: &[uuid::Uuid]) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{
            EventChannel, EventMeta, MessageOrigin, ThreadEvent,
        };

        // Find threads where the LAST exchange boundary (MessageReceived or
        // TriggerStarted) has activity events after it but no terminal
        // event. Returns the originating event id and the originating event's
        // type so the emitted ResponseAborted can carry `request_event_id`
        // linking back to it AND the right channel (chat vs trigger).
        // Tuple shape: (thread_id, partial_text, originating_event_id, originating_event_type, originating_channel).
        type OrphanRow = (
            uuid::Uuid,
            Option<String>,
            Option<uuid::Uuid>,
            Option<String>,
            Option<String>,
        );
        // `ChildThreadCompleted` is a turn-originator alongside
        // `MessageReceived` / `TriggerStarted` — a thread waking from a
        // finished child stamps the CTC's id as `request_event_id` on
        // every event in the wake turn. Without CTC in `last_start` and
        // the LATERAL JOIN, a chat (or CC) thread whose only in-flight
        // turn was woken from a child is silently skipped: `last_start`
        // resolves to a prior completed turn's MR, `last_terminal`
        // (that turn's ResponseGenerated) is newer than `last_start`,
        // and the `last_terminal <= last_start` orphan predicate fails.
        // The thread sits in `requesting` forever with no Continue path.
        // SQL is in the `ORPHAN_THREADS_SQL` const at module top so the
        // sibling test in `recovery_tests` can run the exact same query.
        let rows: Vec<OrphanRow> = match sqlx::query_as(&orphan_threads_sql())
            .bind(exclude_thread_ids)
            .fetch_all(self.pool())
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log!("[Recovery] Failed to query orphaned threads: {}", e);
                return;
            }
        };

        if rows.is_empty() {
            return;
        }

        log!("[Recovery] Found {} orphaned thread(s)", rows.len());

        for (
            thread_id,
            partial_text,
            originating_event_id,
            originating_event_type,
            originating_channel,
        ) in rows
        {
            let text = match partial_text {
                Some(t) if !t.is_empty() => t,
                _ => "This response was interrupted by an engine restart.".to_string(),
            };
            // ChildThreadCompleted-anchored turns inherit the parent thread's
            // emit channel from the CTC row itself (stamped by
            // `notify_parent_of_child_completion`). MR / TriggerStarted callers
            // hardcoded chat/trigger via the originating_event_type below, but
            // the wire channel on those rows is the same — fall back to the
            // event-type mapping when the payload's channel field is missing
            // (legacy rows that predated stamping it on starts).
            let channel = originating_channel
                .as_deref()
                .and_then(EventChannel::from_wire)
                .or(match originating_event_type.as_deref() {
                    Some("TriggerStarted") => Some(EventChannel::Trigger),
                    Some("MessageReceived") => Some(EventChannel::Chat),
                    _ => None,
                });

            // Direct .emit (not emit_response_aborted): wants the Err for the per-thread log below.
            if let Err(e) = self
                .event_bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ResponseAborted {
                        text,
                        images: vec![],
                        model: None,
                        reasoning_effort: None,
                        cause: crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
                    },
                    meta: EventMeta {
                        channel,
                        request_event_id: originating_event_id,
                        actor: Some(MessageOrigin::system()),
                        ..EventMeta::NONE
                    },
                })
                .await
            {
                log!(
                    "[Recovery] Failed to emit ResponseAborted for thread {}: {}",
                    thread_id,
                    e
                );
                continue;
            }

            log!("[Recovery] Recovered orphaned thread {}", thread_id);
        }
    }
}

/// Blast-radius bound on the boot auto-resume. A switch normally interrupts one
/// or two in-flight chat threads; this only bites on a pathological workspace.
/// Anything over the cap is logged by thread id and keeps its manual Continue —
/// a silent truncation would read as "resumed everything".
const MAX_CHAT_SWITCH_RESUMES: usize = 16;

/// The chat / trigger threads a user-initiated *Switch to new version* interrupted
/// and that nothing has resumed since.
///
/// Coding agents get this via `recover_orphaned_worktrees` → `enqueue_switch_resume`;
/// this is the chat half. It cannot reuse `recover_orphaned_threads`: that sweep
/// requires a turn with activity and **no** terminal event (the crash shape), whereas
/// the switch teardown (`abort_in_flight_for_restart`) always lands a
/// `ResponseAborted` first — so a switch-interrupted chat thread is invisible to it.
///
/// The predicate is the same one the coding-agent gate uses, assembled from the
/// shared fragments in `agent_recovery::recovery` so the two can't drift:
///
/// * `SWITCH_TEARDOWN_ABORT_SQL` — a device-attributed `EngineShutdown` abort, the
///   teardown boundary's fingerprint. A crash leaves none → manual Continue.
/// * no newer `THREAD_START_EVENTS_SQL` event — the loop-breaker. `continue_chat`
///   emits `ContinuationStarted`, which is in that set, so a resume that dies before
///   producing anything else is not resumed a second time on the next boot.
///
/// A question-parked thread needs no special case: the preserve guard means no abort
/// was ever emitted for it, so it cannot match.
///
/// Selection is by `source`, deliberately NOT by `is_coding_agent` — that column is
/// separately known to be corruptible (a chat thread's `ContinuationStarted` used to
/// flip it true), and this gate must not inherit that bug. Archived threads are
/// excluded for the same reason `ORPHAN_THREADS_SQL` excludes them: the user closed
/// them, and resuming would revive the row.
fn switch_resume_candidates_sql() -> String {
    // GROUP BY (not DISTINCT) so the result is one row per THREAD. A thread with
    // two unsuperseded switch aborts would otherwise yield two rows and drive
    // `continue_chat` twice — harmless (the second is a no-op via that function's
    // own idempotency check) but a duplicate-work path the query should not
    // express. Oldest interruption first, so a boot resumes in the order the
    // threads were interrupted.
    format!(
        "SELECT e.aggregate_id::uuid AS thread_id, MAX(e.sequence) AS abort_sequence \
         FROM events e \
         JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
         WHERE e.aggregate = 'thread' \
           AND t.source IN ('chat', 'trigger') \
           AND t.state = 'active' \
           AND t.archive_state != 'archived' \
           AND {abort} \
           AND e.sequence > COALESCE(( \
               SELECT MAX(s.sequence) FROM events s \
               WHERE s.aggregate_id = e.aggregate_id \
                 AND s.event_type IN ({starts}) \
           ), 0) \
         GROUP BY e.aggregate_id \
         ORDER BY abort_sequence ASC",
        abort = crate::engine::agent_recovery::SWITCH_TEARDOWN_ABORT_SQL,
        starts = crate::engine::agent_recovery::THREAD_START_EVENTS_SQL,
    )
}

/// Run [`switch_resume_candidates_sql`]. Errors yield an empty list (logged): a
/// transient DB failure must degrade to the manual Continue affordance, never to a
/// panic on the boot path.
pub(crate) async fn switch_resume_candidates(pool: &sqlx::PgPool) -> Vec<uuid::Uuid> {
    match sqlx::query_as::<_, (uuid::Uuid, i64)>(&switch_resume_candidates_sql())
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows.into_iter().map(|(id, _)| id).collect(),
        Err(e) => {
            log!(
                "[Recovery] chat switch-resume candidate scan failed: {} — \
                 affected threads keep their manual Continue",
                e
            );
            Vec::new()
        }
    }
}

impl LucidosEngine {
    /// Auto-resume the chat / trigger threads a user-initiated *Switch to new
    /// version* interrupted — the chat parity of `resume_pending_switches`.
    ///
    /// Drives each candidate through `continue_chat`, the same entry point the
    /// manual **Continue** button uses, so the resumed turn gets the identical
    /// `ContinuationStarted` boundary and side-effect engine note. `actor: None`
    /// matches the coding-agent path's choice: the resume is a recovery
    /// consequence, not a device click, so the boundary keeps the Lucidos-mark
    /// chip rather than reading "You".
    ///
    /// Sequential on purpose — each resume spawns an agentic loop, and a boot-time
    /// fan-out of LLM calls is worth avoiding. `continue_chat` itself returns after
    /// spawning, so this does not wait for the turns to finish.
    ///
    /// Called from `main.rs` AFTER `thread_queue.spawn_settle_subscriber()`: a
    /// resumed chat turn immediately reads as `running`, and the settle subscriber
    /// must be live for that status change to reconcile a queue slot. It also must
    /// follow `recover_orphan_tool_calls`, or the re-entered turn reconstructs an
    /// unpaired `tool_use` block and the provider rejects the call.
    pub async fn resume_pending_chat_switches(self: &Arc<Self>) {
        let mut candidates = switch_resume_candidates(self.pool()).await;
        if candidates.is_empty() {
            return;
        }

        if candidates.len() > MAX_CHAT_SWITCH_RESUMES {
            let skipped = candidates.split_off(MAX_CHAT_SWITCH_RESUMES);
            log!(
                "[Recovery] {} chat thread(s) interrupted by the switch exceed the \
                 per-boot resume cap of {} — these keep their manual Continue: {:?}",
                skipped.len(),
                MAX_CHAT_SWITCH_RESUMES,
                skipped
            );
        }

        log!(
            "[Recovery] Auto-resuming {} chat/trigger thread(s) after a user switch",
            candidates.len()
        );

        for thread_id in candidates {
            match self.continue_chat(thread_id, None).await {
                Ok(outcome) => log!(
                    "[Recovery] chat switch-resume for thread {}: {:?}",
                    thread_id,
                    outcome
                ),
                Err(e) => log!(
                    "[Recovery] chat switch-resume for thread {} failed: {} — \
                     the manual Continue affordance remains",
                    thread_id,
                    e
                ),
            }
        }
    }
}

impl LucidosEngine {
    /// Re-emit a synthetic `ToolResult` for every persisted `ToolCalled` that
    /// has no matching `ToolResult` in the same thread. Mirror of the
    /// `ResponseAborted` recovery sweep above, but at the inner tool layer:
    /// when the engine dies mid-tool, the `ToolCalled` event lands in the
    /// events table but no `ToolResult` ever arrives. On the next LLM call,
    /// the resume builder reconstructs an assistant `tool_use` block for
    /// that orphan, and the Claude API rejects with "tool_use ids were
    /// found without tool_result blocks immediately after".
    ///
    /// Pairing rule lives in `core::store::find_orphan_tool_called_ids` —
    /// shared with the resume builder so both layers identify the same set
    /// of orphans.
    ///
    /// Idempotent: subsequent restarts find no orphans because the synthetic
    /// `ToolResult` we emit on the first pass pairs with the same
    /// `ToolCalled` on the second pass.
    pub async fn recover_orphan_tool_calls(self: &Arc<Self>) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

        // Skip archived threads — same reasoning as the
        // `recover_orphaned_threads` sweep: don't emit follow-up events
        // (here, synthetic ToolResults) on a row the user dismissed.
        // SQL is in the `ORPHAN_TOOL_CALLS_SQL` const at module top so
        // the sibling test in `recovery_tests` can run the exact same
        // query.
        let rows: Vec<EventRow> = match sqlx::query_as::<_, EventRow>(&orphan_tool_calls_sql())
            .fetch_all(self.pool())
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log!("[Recovery] orphan ToolCalled query failed: {}", e);
                return;
            }
        };

        if rows.is_empty() {
            return;
        }

        // Rows are already sorted by `(thread_id, created, sequence)` — walk
        // linearly and flush each thread's events through the shared
        // orphan-detection helper at the thread boundary. Avoids a HashMap
        // regroup and keeps the per-thread `events` slice borrowed.
        let mut total_orphans = 0usize;
        let mut start = 0usize;
        while start < rows.len() {
            let thread_id = match rows[start].thread_id {
                Some(t) => t,
                None => {
                    start += 1;
                    continue;
                }
            };
            let mut end = start + 1;
            while end < rows.len() && rows[end].thread_id == Some(thread_id) {
                end += 1;
            }

            let orphans =
                crate::core::store::find_orphan_tool_called_ids(&rows[start..end]);
            for (orphan_id, tool_name) in orphans {
                total_orphans += 1;
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::ToolResult {
                                name: tool_name,
                                result: format!(
                                    "[Tool execution interrupted by engine restart — original ToolCalled event_id: {}]",
                                    orphan_id
                                ),
                                images: vec![],
                                success: false,
                                // Frontend `groupIntoExchanges` routes this
                                // synthetic ToolResult into the same exchange
                                // as its originating `ToolCalled` by event
                                // id. Without this, the synthetic ToolResult
                                // would route via `request_event_id` to
                                // whichever exchange the redirect points to
                                // (e.g. the new ResponseAborted boundary
                                // exchange) instead of pairing with its
                                // ToolCalled — the "Executing …" spinner on
                                // the original step would keep spinning.
                                tool_called_event_id: Some(orphan_id),
                            },
                            meta: EventMeta {
                                actor: Some(MessageOrigin::system()),
                                ..EventMeta::NONE
                            },
                        },
                        "[Recovery] ToolResult (orphan ToolCalled after restart)",
                    )
                    .await;
            }
            start = end;
        }

        if total_orphans > 0 {
            log!(
                "[Recovery] Emitted {} synthetic ToolResult(s) for orphan ToolCalled events",
                total_orphans
            );
        }
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
