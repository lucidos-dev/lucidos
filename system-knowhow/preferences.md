---
name: Preferences
description: The preferences (Settings) the Lucidos Agent reads and changes with get_preferences / set_preference: theme, language, timezone, push, chat models, UI scale, font and public webhook URL. Includes allowed values, defaults and scope.
---

# Preferences (Settings the agent can change)

Lucidos **Settings** spans several stores. This file covers the
**preferences** store: the key→value settings the *Lucidos Agent* reads and
writes with the grouped **`preferences`** tool (`action: get | set`). The CLI
mirrors it as `lucidos preferences get | set` (see `lucidos-cli.md`). Here,
`preferences(action="get")` / `preferences(action="set", key, value)` are shorthand for
`preferences(action="get")` / `preferences(action="set", key, value)`. The old
flat tool names still work as aliases.

- **`get` (`preferences(action="get")`)**: lists every settable preference with its current
  value, allowed values, default, and scope (global or per-device). Call it when
  you are unsure of a key or a valid value, or suspect a per-device override.
- **`set` (`preferences(action="set", key, value)`)**: changes one preference. `value` is
  always a string (`"true"`/`"false"` for booleans, `"125"` for numbers, the
  exact enum string otherwise).

The other Settings stores have their own tools, never `preferences(action="set")`:

| Want to change… | Use |
|---|---|
| A preference below | `preferences(action="set")` |
| Which models appear in the picker, or a model's routes, preferred provider or context window | `manage_models` |
| An API key / secret | `request_credential` (never put secrets in a preference) |
| A non-secret env var | `env_vars` (`action: set`) |
| A registered repo | `manage_repositories` |
| An MCP server | the `mcp` tool (`setup`, `enable`, `disable`, …) |
| Command-safety (the command guard) | not agent-settable: Settings → Permissions |

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/preferences/how-a-write-propagates` | How a write propagates; Who may write a key; Background models; The memory module; Settable preferences |
| `system-knowhow/preferences/settable-preferences` | Settable preferences (continued) |
| `system-knowhow/preferences/settable-preferences-2` | Settable preferences (continued) |
| `system-knowhow/preferences/read-only-managed-elsewhere` | Read-only / managed elsewhere |
