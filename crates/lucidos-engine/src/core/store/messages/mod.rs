use super::types::*;
use super::EventStore;
use crate::core::EventRow;
use chrono::{DateTime, Utc};

mod build;
mod resume;

pub use build::format_child_thread_completed_block;
pub(crate) use build::build_session_messages;
pub(crate) use resume::{
    build_resume_tool_blocks_with_skip_ids, collect_tool_pairs_chronological,
    find_orphan_tool_called_ids, RESUME_VERBATIM_TOOL_TAIL,
};

impl EventStore {
    /// Get all messages for a specific request (for history time travel)
    /// Get session messages as raw text. HTML conversion happens at the API layer.
    /// Queries directly by request_id using the idx_events_request_id index.
    pub async fn get_request_messages_by_id(
        &self,
        request_event_id: &str,
    ) -> Result<Vec<SessionMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let events = sqlx::query_as::<_, EventRow>(
            r#"
            SELECT id, event_type, payload, created, thread_id, sequence
            FROM events
            WHERE payload->>'request_id' = $1
            ORDER BY created ASC
            "#,
        )
        .bind(request_event_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(build_session_messages(&events))
    }

    /// Get recent messages across all conversations, ordered chronologically (oldest first).
    /// `limit` controls the number of recent **threads** scoped in, not raw events.
    ///
    /// Scopes by thread_id: selects the N threads whose most recent
    /// `MessageReceived` activity is newest (`MAX(created) DESC`), then loads ALL
    /// events belonging to those threads. This prevents cross-thread
    /// contamination — events from scheduled triggers (which have their own
    /// thread_ids) can never leak into chat thread results, regardless of whether
    /// individual events have the correct channel tag.
    ///
    /// The thread-selection CTE orders by recency; the outer query orders the
    /// returned events by `created ASC, sequence ASC` so events that share a
    /// timestamp (streaming bursts land in the same second) keep a stable,
    /// insertion-order sequence within each thread for `build_session_messages`.
    pub async fn get_recent_messages(
        &self,
        limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> Result<Vec<SessionMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let events = if let Some(before_ts) = before {
            sqlx::query_as::<_, EventRow>(
                r#"
                WITH recent_threads AS (
                    SELECT thread_id
                    FROM events
                    WHERE event_type = 'MessageReceived'
                      AND thread_id IS NOT NULL
                      AND created < $2
                    GROUP BY thread_id
                    ORDER BY MAX(created) DESC
                    LIMIT $1
                )
                SELECT e.id, e.event_type, e.payload, e.created, e.thread_id, e.sequence
                FROM events e
                WHERE e.thread_id IN (SELECT thread_id FROM recent_threads)
                  AND e.event_type IN ('MessageReceived', 'Thinking', 'ThoughtStreamed', 'MemorySearched', 'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed', 'TextStreamed', 'ToolCalled', 'ToolResult')
                  AND e.created < $2
                ORDER BY e.created ASC, e.sequence ASC
                "#,
            )
            .bind(limit)
            .bind(before_ts)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, EventRow>(
                r#"
                WITH recent_threads AS (
                    SELECT thread_id
                    FROM events
                    WHERE event_type = 'MessageReceived'
                      AND thread_id IS NOT NULL
                    GROUP BY thread_id
                    ORDER BY MAX(created) DESC
                    LIMIT $1
                )
                SELECT e.id, e.event_type, e.payload, e.created, e.thread_id, e.sequence
                FROM events e
                WHERE e.thread_id IN (SELECT thread_id FROM recent_threads)
                  AND e.event_type IN ('MessageReceived', 'Thinking', 'ThoughtStreamed', 'MemorySearched', 'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed', 'TextStreamed', 'ToolCalled', 'ToolResult')
                ORDER BY e.created ASC, e.sequence ASC
                "#,
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(build_session_messages(&events))
    }
}

/// Walk every `ContextDismissed` event in the stream and collect the set of
/// dismissed event ids. The agent emits these via the `dismiss_from_context`
/// tool — see `engine/tools/mod.rs::execute_dismiss_from_context`. The
/// resume helper consults this set to drop both `(ToolCalled, ToolResult)`
/// pairs (matched on the `ToolCalled.id`) and `ChildThreadCompleted` blocks.
fn collect_dismissed_event_ids(events: &[EventRow]) -> std::collections::HashSet<String> {
    let mut dismissed = std::collections::HashSet::new();
    for event in events {
        if event.event_type == "ContextDismissed" {
            if let Some(id) = event
                .payload
                .get("dismissed_event_id")
                .and_then(|v| v.as_str())
            {
                dismissed.insert(id.to_string());
            }
        }
    }
    dismissed
}

#[cfg(test)]
#[path = "../messages_tests/helpers.rs"]
mod msg_helpers;

#[cfg(test)]
#[path = "../messages_tests/response_state.rs"]
mod response_state_tests;

#[cfg(test)]
#[path = "../messages_tests/text_chunks.rs"]
mod text_chunks_tests;

#[cfg(test)]
#[path = "../messages_tests/events.rs"]
mod events_tests;

#[cfg(test)]
#[path = "../messages_tests/message_building.rs"]
mod message_building_tests;

#[cfg(test)]
#[path = "../messages_tests/resume_blocks.rs"]
mod resume_blocks_tests;

#[cfg(test)]
#[path = "../messages_tests/resume_orphans.rs"]
mod resume_orphans_tests;

#[cfg(test)]
#[path = "../messages_tests/recent_messages.rs"]
mod recent_messages_tests;
