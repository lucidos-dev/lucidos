---
name: Preferences
description: The user preferences (Settings) the Lucidos Agent can read and change with get_preferences / set_preference — theme, language, timezone, push, the welcome message, chat model, reasoning effort, UI scale, font, and more. Covers the catalog of settable keys with their allowed values, defaults, and global-vs-device scope; how a write propagates (the persisted PreferencesChanged / LanguageSet / TimezoneSet events that open pages live-apply); the device-scope override trap; and which Settings live in OTHER stores (models via manage_models, secrets via request_credential, the command guard which is human-only). Load when the user asks to change a setting, toggle dark mode, show/hide the welcome message, set their language/timezone, or switch chat model.
---

# Preferences (Settings the agent can change)

Lucidos **Settings** spans several backing stores. This file covers the
**preferences** store — the key→value settings the *Lucidos Agent* reads and
writes with the grouped **`preferences`** tool (`action: get | set`). The CLI
mirrors it as `lucidos preferences get | set` (see `lucidos-cli.md`). Throughout
this file, `get_preferences` / `set_preference(key, value)` are shorthand for
`preferences(action="get")` / `preferences(action="set", key, value)`; the old
flat tool names still work as back-compat aliases.

- **`get` (`get_preferences`)** — list every settable preference with its current
  value, allowed values, default, and scope (global vs per-device). Call it when
  you're unsure of a key, a valid value, or whether a per-device override is
  shadowing the global value.
- **`set` (`set_preference(key, value)`)** — change one preference. `value` is
  always a string (`"true"`/`"false"` for booleans, `"125"` for numbers, the
  exact enum string otherwise).

The other Settings stores have their own tools — don't try to reach them through
`set_preference`:

| Want to change… | Use |
|---|---|
| A preference below | `set_preference` |
| Which models appear in the picker | `manage_models` |
| An API key / secret | `request_credential` (never put secrets in a preference) |
| A non-secret env var | `env_vars` (`action: set`) |
| A registered repo | `manage_repositories` |
| An MCP server | `setup_mcp_server` / `start_mcp_server` / … |
| Command-safety (the command guard) | not agent-settable — Settings → Permissions |

## How a write propagates

`set_preference` validates against the **preference catalog**
(`crates/lucidos-engine/src/core/preference_catalog.rs` — the single source of
truth) and writes through the engine's one preference chokepoint, which emits the
persisted **`PreferencesChanged`** event (or `LanguageSet` / `TimezoneSet` for
locale). Open Lucidos pages live-apply on those events, so a change the agent
makes shows up without a reload. No transient event, no restart.

**Device scope.** Device-scoped keys (theme, font-family, ui-scale,
push_notifications) are stored per-device and override the global value on the
device that set them. `set_preference` automatically targets the calling device —
you never pass a device id. This is the trap to remember: setting `theme=dark`
globally does nothing on a device that has its own `theme=light` override. Use
`get_preferences` to see the per-device effective value vs the global one.

## Settable preferences

| Key | Scope | Allowed values | Default | What it does |
|---|---|---|---|---|
| `language` | global | text | (detected from conversation) | Language for responses + session summaries (e.g. "English", "Norwegian"). |
| `timezone` | global | IANA timezone | (unset) | Timezone for triggers + time display (e.g. "Europe/Oslo"). Set before creating triggers. |
| `chat_model` | global | a model id from the registry | `claude-opus-4-8@default` | Active chat model. Use `manage_models(action='list')` to see options. |
| `chat_reasoning_effort` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `high` | Thinking budget (clamped per model). |
| `image_model` | global | `auto` \| `imagen-4` \| `gpt-image-1` \| `gpt-image-1.5` \| `gpt-image-2` | `auto` | Model used by `generate_image`. |
| `model_title` | global | a model id | `gemini-3-flash-preview` | Background model for thread titles. |
| `model_image_description` | global | a model id | `gemini-3-flash-preview` | Background model that describes uploaded images. |
| `model_memory` | global | a model id | `gemini-3-flash-preview` | Background model for memory extraction. |
| `vertex_region` | global | text | `europe-west1` | Google Vertex AI region. |
| `local_base_url` | global | URL | `http://localhost:11434/v1` | Base URL for the `local` OpenAI-compatible provider. |
| `notifications_filter` | global | `all` \| `unread` | `all` | Which notifications the bell shows. |
| `mobile_header_sticky` | global | `true` \| `false` | `true` | Keep the mobile header always visible. |
| `welcome_suggestions_dismissed` | global | `true` \| `false` | `false` | Hide the new-workspace welcome message. Set `false` to SHOW it again. |
| `coding_agent_default` | global | `claude-code` \| `codex` | `claude-code` | Default coding agent the compose picker pre-selects. |
| `backup_schedule` | global | 6-field cron (in the user's timezone) or `off` | `off` | Automatic backup schedule. E.g. `0 0 3 * * *` = daily 03:00, `0 0 3 * * 0` = weekly Sun 03:00, `0 0 */12 * * *` = every 12h. Fires in the user's `timezone`. Requires `backup_provider` set + connected. |
| `backup_provider` | global | `google_drive` \| `dropbox` | (unset) | Cloud destination. The account must be connected in Settings → System → Backup (OAuth) before a scheduled backup can upload. |
| `backup_retention` | global | number 1–50 | `5` | How many recent backups to keep; older ones are pruned after each successful backup. |
| `theme` | device | `light` \| `dark` \| `system` | `dark` | Color theme for the calling device. |
| `font-family` | device | `monospace` \| `system` \| `inter` \| `jetbrains-mono` \| `ibm-plex-mono` \| `fira-code` | `monospace` | UI font for the calling device. `fira-code` also enables programming ligatures. |
| `ui-scale` | device | number 75–200 | `100` | UI scale percent for the calling device (snaps to 12.5 steps). |
| `push_notifications` | device | `enabled` \| `declined` | (unset) | Push notifications for the calling device. `enabled` triggers the OS/browser permission prompt. |

## Read-only / managed elsewhere

`get_preferences` also surfaces these so you can explain them, but
`set_preference` refuses them (it returns a hint pointing at the right Settings
surface):

- `command_guard`, `command_guard_judge`, `model_command_judge` — the command
  guard (safety gate over the agent's own bash/python). Settings → Permissions
  only. You must not disable your own safety gate.
- `backup_last_run` — internal backup state (the last run's outcome), not a
  setting. The backup *schedule*, *provider*, and *retention* ARE settable (see
  the table above); use `get_backup_status` to read the current schedule, next /
  last run, and recent history.
- `keybindings` — Settings → Keyboard Shortcuts.
- `capture_context` — a debug-only toggle.

> Keep this file in lockstep with `core/preference_catalog.rs` — a `cargo test`
> sync test fails if a catalog key is missing here (see
> `.claude/rules/system-knowhow.md`).
