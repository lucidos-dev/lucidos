-- Add `mcp_launch_approvals`: which MCP server launch lines someone approved.
--
-- `fingerprint` is the hex HMAC-SHA256, under the secret store key, of the
-- server's real launch line (command, args, env and spawn directory). The key
-- keeps a sealed env value from being guessed. With the command guard on, the engine
-- refuses to start a server whose current fingerprint has no row here. A
-- re-registration that changes the line therefore needs a new approval.
--
-- No foreign key: the command guard approves a setup's line before the setup
-- writes the server row. Removing a server deletes its approvals.
--
-- No backfill. Approving every existing line would bless exactly the rows the
-- gate distrusts: each was written by a model call.

CREATE TABLE mcp_launch_approvals (
  server_id TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  approved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (server_id, fingerprint)
);
