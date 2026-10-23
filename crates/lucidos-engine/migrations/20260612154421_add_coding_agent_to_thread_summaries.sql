-- Which coding-agent backend drives this thread: 'claude-code' | 'codex'
-- (kebab-case, same wire values as the CodingAgent enum). NULL = legacy row
-- from before the column existed — every such thread was Claude Code, and
-- consumers default NULL via CodingAgent::parse. Distinct from
-- coding_agent_kind, which is the thread *flavor* (lucidos | app | external);
-- a thread of any kind can run on any backend. Locked in by the first
-- SessionStarted (COALESCE keeps the existing value), mirroring cc_repo_id.
ALTER TABLE thread_summaries ADD COLUMN coding_agent TEXT;
