-- A call makes the thread real (ADR 0167), and this rescues the ones it
-- already left behind. Until the projection arms shipped, only a delegated
-- utterance promoted a draft, so a call the talker answered alone stranded its
-- whole transcript inside the compose view.
--
-- Keyed on a SPOKEN row rather than on a session start, matching the arms: a
-- call that connected and said nothing is still a draft, and a draft is
-- something the reader can see and discard.
--
-- `has_response` travels with the promotion. It is the listing gate
-- `get_recent_threads` filters on, so a promoted thread without it is neither
-- a draft nor a listed thread.
--
-- Idempotent by construction: a promoted row no longer reads 'composing', and
-- first_message is filled only where it is still null. Nothing here touches
-- status, source or message_count, which belong to a turn the call never ran.

UPDATE thread_summaries t
   SET state = 'active',
       has_response = TRUE,
       compose_text = '',
       compose_images = '[]'::jsonb,
       compose_mode = NULL,
       compose_selection = NULL,
       compose_epoch = t.compose_epoch + 1
 WHERE t.state = 'composing'
   AND EXISTS (
       SELECT 1 FROM events e
        WHERE e.thread_id = t.thread_id
          AND e.event_type IN ('SpokenMessageReceived', 'SpokenReplyGenerated')
   );

-- Every thread a call ever spoke on, draft or not. A thread promoted long ago
-- by a delegation already reads TRUE, and one that never delegated needs this
-- to reach the drawer at all.
UPDATE thread_summaries t
   SET has_response = TRUE
 WHERE t.has_response = FALSE
   AND EXISTS (
       SELECT 1 FROM events e
        WHERE e.thread_id = t.thread_id
          AND e.event_type IN ('SpokenMessageReceived', 'SpokenReplyGenerated')
   );

-- The caller's first words, so the drawer row reads as something rather than
-- "Untitled Thread". `format_display_title` falls back to this column, and a
-- call nobody delegated from writes no other message to take it from.
UPDATE thread_summaries t
   SET first_message = spoken.text
  FROM (
       SELECT DISTINCT ON (e.thread_id)
              e.thread_id,
              e.payload->>'text' AS text
         FROM events e
        WHERE e.event_type = 'SpokenMessageReceived'
          AND COALESCE(e.payload->>'text', '') <> ''
        ORDER BY e.thread_id, e.created, e.sequence
       ) AS spoken
 WHERE t.thread_id = spoken.thread_id
   AND t.first_message IS NULL;
