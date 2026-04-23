use std::sync::Arc;

use crate::engine::CognosEngine;

impl CognosEngine {
    /// Detect threads whose last exchange has activity events but no terminal event.
    /// These are in-flight threads (chat or CC) that died when the engine crashed.
    /// Handles both first-exchange orphans (has_response=false) and multi-exchange
    /// orphans where a previous exchange completed but the latest was interrupted
    /// (e.g., lid close during tool execution after a prior ResponseGenerated).
    ///
    /// Emits ResponseAborted via EventBus with partial text so the thread is
    /// marked UNREAD and the user notices.
    ///
    /// `exclude_thread_ids` — threads being actively recovered by worktree recovery;
    /// they should not be aborted here since CC recovery will handle them.
    pub async fn recover_orphaned_threads(self: &Arc<Self>, exclude_thread_ids: &[uuid::Uuid]) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        // Find threads where the LAST exchange boundary (MessageReceived or
        // TriggerStarted) has activity events after it but no terminal
        // event. This correctly handles multi-exchange threads where earlier
        // exchanges completed (has_response=true) but the latest was interrupted.
        let rows: Vec<(uuid::Uuid, Option<String>)> = match sqlx::query_as(
            r#"
            WITH candidate_threads AS (
                SELECT thread_id::text AS aggregate_id
                FROM thread_summaries
                WHERE thread_id != ALL($1::uuid[])
            ),
            per_thread AS (
                SELECT
                    e.aggregate_id,
                    MAX(CASE WHEN e.event_type IN ('MessageReceived','TriggerStarted')
                             THEN e.created END) AS last_start,
                    MAX(CASE WHEN e.event_type IN ('TextStreamed','Thinking','ToolCalled','ToolResult',
                                                    'CodingAgentTextStreamed','CodingAgentToolCalled','CodingAgentToolResult')
                             THEN e.created END) AS last_activity,
                    MAX(CASE WHEN e.event_type IN ('ResponseGenerated','ResponseCanceled','ResponseAborted','ResponseFailed',
                                                    'CodingAgentIdled','SessionEnded')
                             THEN e.created END) AS last_terminal
                FROM events e
                WHERE e.aggregate = 'thread'
                  AND e.aggregate_id IN (SELECT aggregate_id FROM candidate_threads)
                  AND e.event_type IN (
                      'MessageReceived','TriggerStarted',
                      'TextStreamed','Thinking','ToolCalled','ToolResult',
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
                      AND e2.created > o.last_start) AS partial_text
            FROM orphans o
            "#,
        )
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

        for (thread_id, partial_text) in rows {
            let text = match partial_text {
                Some(t) if !t.is_empty() => t,
                _ => "This response was interrupted by an engine restart.".to_string(),
            };

            if let Err(e) = self
                .event_bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ResponseAborted {
                        text,
                        images: vec![],
                        model: None,
                        reasoning_effort: None,
                    },
                    meta: EventMeta::NONE,
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
