-- MCP server configurations for the generic MCP client.
-- Stores connection details for MCP servers that the LLM can set up and use.

CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    args JSONB NOT NULL DEFAULT '[]',
    env JSONB NOT NULL DEFAULT '{}',
    auto_approve BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
