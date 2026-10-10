-- Recompute active_children_count from actual event data.
-- Previously, ResponseCanceled/ResponseAborted on child threads did not
-- decrement the parent's counter, causing parents to stay stuck in "In Progress".
--
-- Strategy: count children that have NO terminal event as "still active".
-- A child with any terminal event (ResponseGenerated, ResponseCanceled, etc.)
-- is considered done and should not contribute to the parent's active count.
UPDATE thread_summaries p
SET active_children_count = (
    SELECT COUNT(*)
    FROM thread_summaries child
    WHERE child.parent_thread_id = p.thread_id
      AND NOT EXISTS (
          SELECT 1 FROM events e
          WHERE e.aggregate_id = child.thread_id::text
            AND e.event_type IN (
                'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted',
                'ResponseFailed', 'ClaudeCodeIdled', 'SessionEnded'
            )
      )
)
WHERE p.active_children_count > 0;
