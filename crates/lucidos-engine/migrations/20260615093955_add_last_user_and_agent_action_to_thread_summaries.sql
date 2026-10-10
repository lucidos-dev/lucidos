-- Split the single `last_activity` recency signal into two attributable ones:
--   last_user_action  — when the USER last drove this thread forward
--   last_agent_action — when the AGENT (or trigger) last did something
-- `last_activity` stays as the max-of-both "last anything happened" signal and
-- still drives the relative-time display elsewhere; the drawer now SORTS by
-- last_user_action so background agent churn no longer reshuffles the list.
-- Both are NOT NULL so every consumer (sort key, tooltip) has a value; new rows
-- default to NOW() (≈ creation), and the bumps below keep them current.
ALTER TABLE thread_summaries
    ADD COLUMN IF NOT EXISTS last_user_action TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ADD COLUMN IF NOT EXISTS last_agent_action TIMESTAMPTZ NOT NULL DEFAULT NOW();

-- Backfill last_user_action: most-recent user-initiated event per thread, else
-- the creation default. `mode` defaults to 'human' so legacy MessageReceived
-- rows persisted before the field existed still read as user messages. The
-- ClaudeCode* aliases catch rows written before the CodingAgent* rename.
UPDATE thread_summaries ts SET last_user_action = sub.t
FROM (
    SELECT aggregate_id, MAX(created) AS t
    FROM events
    WHERE aggregate_id IS NOT NULL
      AND (
          (event_type = 'MessageReceived' AND COALESCE(payload->>'mode', 'human') = 'human')
          OR event_type IN (
              'CodingAgentUserMessageSent', 'ClaudeCodeUserMessageSent',
              'UserPromptInjected', 'UserQuestionAnswered',
              'CodingAgentPermissionResolved', 'ClaudeCodePermissionResolved',
              'CommandPermissionResolved', 'ChangeApplied', 'ChangeDiscarded'
          )
      )
    GROUP BY aggregate_id
) sub
WHERE ts.thread_id::text = sub.aggregate_id;

-- Backfill last_agent_action: most-recent agent/trigger-driven event per thread,
-- else the creation default. Mirrors the projection's per-arm bumps below: an
-- automated (non-human) MessageReceived counts as agent activity.
UPDATE thread_summaries ts SET last_agent_action = sub.t
FROM (
    SELECT aggregate_id, MAX(created) AS t
    FROM events
    WHERE aggregate_id IS NOT NULL
      AND (
          (event_type = 'MessageReceived' AND COALESCE(payload->>'mode', 'human') <> 'human')
          OR event_type IN (
              'TriggerStarted', 'TriggerCompleted',
              'ResponseGenerated', 'ResponseAborted', 'ResponseFailed',
              'CodingAgentIdled', 'ClaudeCodeIdled',
              'ToolCalled', 'ToolResult', 'TextStreamed', 'ThoughtStreamed', 'Thinking',
              'MemorySearched',
              'CodingAgentTextStreamed', 'ClaudeCodeTextStreamed',
              'CodingAgentToolCalled', 'ClaudeCodeToolCalled',
              'CodingAgentToolResult', 'ClaudeCodeToolResult',
              'UserQuestionAsked',
              'CodingAgentPermissionRequest', 'ClaudeCodePermissionRequest',
              'CommandPermissionRequested'
          )
      )
    GROUP BY aggregate_id
) sub
WHERE ts.thread_id::text = sub.aggregate_id;

-- Sort indexes mirroring the existing last_activity ones — the drawer's
-- saved/recent/older queries now ORDER BY last_user_action (plain + per-source).
CREATE INDEX IF NOT EXISTS idx_thread_summaries_last_user_action
    ON thread_summaries (last_user_action DESC);
CREATE INDEX IF NOT EXISTS idx_thread_summaries_source_user_action
    ON thread_summaries (source, last_user_action DESC);
