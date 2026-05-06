use std::sync::Arc;

use crate::engine::LucidosEngine;

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
        let rows: Vec<(uuid::Uuid, Option<String>, Option<uuid::Uuid>, Option<String>)> = match sqlx::query_as(
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
                      AND e2.created > o.last_start) AS partial_text,
                   start_evt.id AS originating_event_id,
                   start_evt.event_type AS originating_event_type
            FROM orphans o
            LEFT JOIN LATERAL (
                SELECT e3.id, e3.event_type FROM events e3
                WHERE e3.aggregate_id = o.thread_id::text
                  AND e3.event_type IN ('MessageReceived','TriggerStarted')
                  AND e3.created = o.last_start
                ORDER BY e3.sequence DESC LIMIT 1
            ) start_evt ON true
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

        for (thread_id, partial_text, originating_event_id, originating_event_type) in rows {
            let text = match partial_text {
                Some(t) if !t.is_empty() => t,
                _ => "This response was interrupted by an engine restart.".to_string(),
            };
            let channel = match originating_event_type.as_deref() {
                Some("TriggerStarted") => Some(EventChannel::Trigger),
                Some("MessageReceived") => Some(EventChannel::Chat),
                _ => None,
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
