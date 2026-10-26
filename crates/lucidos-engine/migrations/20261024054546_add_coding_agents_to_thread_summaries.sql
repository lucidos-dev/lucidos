-- Every coding-agent backend a thread's sessions started on ('claude-code' |
-- 'codex'), in first-use order, each once. `coding_agent` stays the thread's
-- current backend (the newest SessionStarted). Empty = a thread that never
-- started a session.
ALTER TABLE thread_summaries ADD COLUMN coding_agents TEXT[] NOT NULL DEFAULT '{}';

-- Each SessionStarted names its backend. A payload without `coding_agent` is a
-- session from before the field existed, and every such session was Claude
-- Code. Deriving from every thread, not only handed-off ones, seeds legacy
-- rows too, so a later handoff appends to `{claude-code}` rather than to `{}`.
UPDATE thread_summaries ts
SET coding_agents = s.agents
FROM (
    SELECT aggregate_id, array_agg(agent ORDER BY first_started) AS agents
    FROM (
        SELECT aggregate_id,
               COALESCE(payload->>'coding_agent', 'claude-code') AS agent,
               min(created) AS first_started
        FROM events
        WHERE event_type = 'SessionStarted' AND aggregate = 'thread'
        GROUP BY aggregate_id, COALESCE(payload->>'coding_agent', 'claude-code')
    ) per_agent
    GROUP BY aggregate_id
) s
WHERE ts.thread_id::text = s.aggregate_id;

-- The current backend is always in the list, even for a row whose events
-- no longer name it.
UPDATE thread_summaries
SET coding_agents = array_append(coding_agents, coding_agent)
WHERE coding_agent IS NOT NULL AND NOT (coding_agent = ANY(coding_agents));
