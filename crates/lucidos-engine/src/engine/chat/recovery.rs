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
        let rows: Vec<OrphanRow> = match sqlx::query_as(ORPHAN_THREADS_SQL)
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
        let rows: Vec<EventRow> = match sqlx::query_as::<_, EventRow>(ORPHAN_TOOL_CALLS_SQL)
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
