-- Add `enabled_mcp_servers` to `thread_summaries` (ADR 0430).
--
-- The MCP server ids this thread switched on for itself. A running server's
-- tools reach only the threads that list it here. `ThreadMcpServerEnabled`
-- adds an id, and `ThreadMcpServerDisabled` removes it. `McpServerStopped`
-- and `McpServerRemoved` remove it from every row.
--
-- No backfill. Before this column every running server reached every thread,
-- and nothing recorded which thread used it.

ALTER TABLE thread_summaries
  ADD COLUMN enabled_mcp_servers TEXT[] NOT NULL DEFAULT '{}';
