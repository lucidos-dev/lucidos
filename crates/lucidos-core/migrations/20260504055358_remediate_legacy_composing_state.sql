-- Fix legacy threads stuck in `composing` state, and reverse user discards
-- that only happened because of the same misclassification.
--
-- 20260503215740_threadcomposestate.sql added the `state` column with default
-- 'composing' and only set 'active' for rows with `message_count > 0`. That
-- check missed engine-spawned CC threads (merge-conflict resolution, missing-
-- harden retrigger) — their projection inserts `message_count = 0` because
-- those threads start from CodingAgentPromptSent, not MessageReceived. The
-- bug surfaced as completed merge-conflict threads appearing in the drafts
-- list and rendering "Messages could not be displayed" when opened (the
-- frontend exchange-builder fix lives in store/thread-events.ts).
--
-- Any thread with at least one pre-ThreadDiscarded event that ISN'T part of
-- the draft lifecycle (ThreadStarted / DraftSaved / ThreadDiscarded /
-- DraftDiscarded) has had real engine activity and belongs in 'active'. True
-- drafts only carry those four event types and stay in 'composing'. The
-- "pre-ThreadDiscarded" qualifier matters because the projection lets
-- post-discard MessageReceived rows persist (rejected by the resurrection
-- guard) — counting them would mistake a legitimately-discarded draft for a
-- misclassified active thread.
UPDATE thread_summaries ts
   SET state = 'active'
 WHERE ts.state IN ('composing', 'discarded')
   AND EXISTS (
     SELECT 1 FROM events e
      WHERE e.aggregate_id = ts.thread_id::text
        AND e.event_type NOT IN (
              'ThreadStarted',
              'DraftSaved',
              'ThreadDiscarded',
              'DraftDiscarded'
            )
        AND NOT EXISTS (
          SELECT 1 FROM events disc
           WHERE disc.aggregate_id = e.aggregate_id
             AND disc.event_type IN ('ThreadDiscarded', 'DraftDiscarded')
             AND disc.sequence <= e.sequence
        )
   );

-- DELETE /api/v1/threads/:id rejects discard-from-active with 409, so any row
-- still in `discarded` after this migration is a legitimately-discarded draft
-- and stays terminal.
