-- Cache each MCP server's tool manifest on its registry row.
--
-- Only a RUNNING server advertised its tools, so a stopped one reported zero
-- tools and zero cost. That is indistinguishable from a server that genuinely
-- costs nothing, and it left no way to answer "what would this cost if I
-- switched it on". The manifest is written after a successful connect and read
-- back whenever the process is not up.
--
-- `tools` holds the McpTool array verbatim (name, description, inputSchema),
-- not sizes: a description has to be renderable, and the schema-deferral
-- catalog needs the real definitions.
--
-- `tools_observed_at` is the freshness signal. NULL means never observed, which
-- every existing row reads as, so nothing claims a manifest it does not have.
--
-- `disabled_tools` holds WIRE names (`mcp__<server>__<tool>`), the canonical
-- spelling everything else keys on: the permission allowlist, dispatch, and the
-- tool definitions the request carries.

ALTER TABLE mcp_servers
    ADD COLUMN IF NOT EXISTS tools JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN IF NOT EXISTS tools_observed_at TIMESTAMPTZ NULL,
    ADD COLUMN IF NOT EXISTS disabled_tools TEXT[] NOT NULL DEFAULT '{}';
