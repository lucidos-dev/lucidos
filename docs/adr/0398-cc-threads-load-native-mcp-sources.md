# 0398: Claude Code threads load Claude Code's own MCP sources, plugins included: no --strict-mcp-config

- **Status**: Accepted
- **Date**: 2026-10-09

## Context

The engine spawned `claude` with `--mcp-config` (the `lucidos_perm` permission
server) and `--strict-mcp-config`. The strict flag makes Claude Code ignore
every other MCP source. So a coding-agent thread never started a plugin's MCP
server: a user with the Slack plugin enabled handed a thread a Slack link, and
ToolSearch found no Slack tools. Adding `mcp__plugin_slack_slack` to the
workspace's `cc-allowed-tools` reached `--allowedTools` and changed nothing,
since the server never ran.

The flag arrived with the permission-prompt wiring
(`docs/plans/2026-04-20-cc-permission-prompt-tool.md`). Its whole rationale was
one line: "keeps things isolated from the user's global MCP config". No ADR
recorded it, and nothing depends on the isolation. Before that change, threads
loaded every MCP source.

Measured against Claude Code 2.1.280 in headless stream-json mode:

| Check | Result |
|---|---|
| Strict flag off | Enabled plugin servers start (`source: plugin`) beside `lucidos_perm` (`source: dynamic`). |
| Startup | `--mcp-config` servers connect "fully async (nonblocking)". The turn does not wait. |
| Plugin tool not on `--allowedTools` | The call reaches the permission-prompt tool, and its deny holds. |
| Plugin tool under a server rule on `--allowedTools` | The call runs with no permission-prompt call. |
| Server exits non-zero at startup | Status `failed`, no tools, and the turn completes. |
| A repo's `.mcp.json` | Its servers start with no trust prompt: headless mode trusts the project. |
| A repo server named like the `--mcp-config` server | The `--mcp-config` one wins. |
| `enableAllProjectMcpServers: false`, `enabledMcpjsonServers: []` | No effect headless. Only `deniedMcpServers`, by name, blocks a server. |

## Decision

The engine stops passing `--strict-mcp-config`. Claude Code loads its own MCP
sources (enabled plugins, user scope, claude.ai connectors, the project
`.mcp.json`) beside the engine's `lucidos_perm`, which stays in `--mcp-config`.

## Rationale

A coding-agent thread is the user's Claude Code, so it should reach what the
user installed into Claude Code. The permission gate never depended on the
strict flag. Every MCP tool call is checked against `--allowedTools` (the
workspace's `cc-allowed-tools`), and anything else goes to `lucidos_perm` as a
card. A plugin's own startup gate keeps working too: a launcher that exits
non-zero leaves the plugin with no tools, not a broken thread.

A repo's `.mcp.json` now starts unprompted. That adds no new trust: the same
headless session already runs the repo's `.claude/settings.json` hooks at
SessionStart, unprompted, and the tools those servers expose still meet the
gate. A repo also cannot replace the permission server, because the
`--mcp-config` server wins a name collision.

## Consequences

- Plugin MCP tools work in threads. Allowlist one server with its server rule,
  `mcp__plugin_<plugin>_<server>`.
- Each Claude Code session, side questions included, starts the enabled plugin
  servers. That is the same cost as a terminal session.
- Claude Code's own per-server caches apply. A server that recently failed or
  needed auth is skipped for about 15 minutes, as in a terminal.
- `MCP_TIMEOUT` (24 hours, for the permission wait) is also Claude Code's connect
  timeout. A server that hangs at startup stays pending without blocking a turn.
- The `build_command` test asserts the flag is absent, so re-adding it for
  "isolation" fails with this ADR's reason.

## Alternatives considered

- **Keep the strict flag and merge enabled plugins into `--mcp-config`.**
  Rejected: it re-implements Claude Code's plugin resolution. That means
  `enabledPlugins` across settings scopes, `installed_plugins.json`, versioned
  cache paths, inline `plugin.json` servers, `${CLAUDE_PLUGIN_ROOT}`, the
  `plugin:<plugin>:<server>` naming and the account pin's `CLAUDE_CONFIG_DIR`.
  Any Claude Code release could drift it silently.
- **Drop the flag, but deny the servers named in the worktree's `.mcp.json`.**
  Rejected: the trust it protects is already granted to the repo's hooks.
  `deniedMcpServers` matches by name, so it would also deny a same-named user or
  plugin server. It also needs a per-spawn settings value.
- **Allowlist servers with `allowedMcpServers`.** Rejected: the engine would need
  every plugin server's name, the same resolution problem as merging.
