---
name: Preferences
description: The preferences (Settings) the Lucidos Agent reads and changes with get_preferences / set_preference: theme, language, timezone, push, welcome message, chat model, reasoning effort, UI scale, font and more, with allowed values, defaults and global-vs-device scope.
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
| Which models appear in the picker, or a model's context window | `manage_models` |
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

**`chat_model` / `chat_reasoning_effort` are the default for NEW threads only.**
These two are the exception to "shows up right away everywhere": a Lucidos Agent
thread reuses the model + reasoning effort it last ran with (*per-thread model
memory*), so a thread that's already running — **including the thread you're in
when you make the change** — keeps its current model/effort (whatever it last
used), independent of this preference. Changing `chat_model` does NOT switch the
current/running thread's model on its next turn; it only sets the fallback a
brand-new thread's first message uses. (Resolution order per turn: explicit
per-request override → the thread's last recorded value → this preference →
provider default.) The one way to change a *running* thread's model/effort is its
in-thread model picker in the compose bar, which writes a per-thread value and
never touches this account default.

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
| `chat_model` | global | a model id from the registry | `claude-opus-5@default` | Default chat model for NEW threads (a running thread reuses its own last-used model — see "How a write propagates"). Use `manage_models(action='list')` to see options. |
| `chat_reasoning_effort` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `high` | Default thinking budget for NEW threads (a running thread reuses its own last-used effort; clamped per model). |
| `image_model` | global | `auto` \| `imagen-4` \| `gpt-image-1` \| `gpt-image-1.5` \| `gpt-image-2` | `auto` | Model used by `generate_image`. |
| `model_title` | global | a model id | `gemini-3-flash-preview` | Background model for thread titles. |
| `reasoning_title` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for title generation. Naming a thread needs none of it. |
| `model_image_description` | global | a model id | `gemini-3-flash-preview` | Background model that describes uploaded images. |
| `reasoning_image_description` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for describing an image. Captioning is perception, so the default spends nothing on deliberation. |
| `model_memory` | global | a model id | `gemini-3-flash-preview` | Background model for the two memory calls every turn makes: extracting facts, and classifying what the turn needs retrieved. It no longer writes the conversation summary (see `model_conversation_summary`). |
| `reasoning_memory` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for fact extraction and query classification. Both return short JSON on every turn. |
| `model_conversation_summary` | global | a model id | (the `model_memory` model) | Background model that writes a thread's *conversation summary*: the paragraph standing in for its older assistant turns. Split out of `model_memory`, so it inherits that value until you set this one. Its input can be 80k tokens, far larger than any other background call. |
| `reasoning_conversation_summary` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `low` | Thinking budget for the conversation summary. Measured output length does not track this setting, so raising it is unlikely to help: the summariser's failures are calls that never complete. |
| `voice_enabled` | global | `true` \| `false` | `false` | Off by default, and experimental. `true` puts a call control in the composer and lets `/api/v1/voice` accept a socket, so a thread can be spoken to. It rents a speech-to-speech talker and needs the OpenAI provider configured. Every spoken utterance starts an ordinary agent turn, so a short call can cost several. With this off nothing voice-shaped is reachable and the two keys below do nothing. |
| `model_voice_talker` | global | a model id | `gpt-realtime` | Speech-to-speech model a *voice session* speaks through. It holds the conversation and nothing else: it has no tools and can change nothing, so every action still goes through the ordinary agent. Deliberately not a chat-model registry row, because a realtime model cannot serve an ordinary turn. |
| `voice_resident_sections` | global | comma-separated section ids | `who-and-where,this-thread,workspace-shape` | What a *voice session* loads at the start of a call. That block is the whole of what voice answers with no wait, since the talker cannot look anything up: everything else waits for the agent. `who-and-where` is the workspace name, timezone, language and local time; `this-thread` is the title and recent turns; `workspace-shape` is the names of apps, triggers and unread notifications. An unknown id is ignored with a log line. |
| `vertex_region` | global | text | `europe-west1` | Google Vertex AI region. |
| `local_base_url` | global | URL | `http://localhost:11434/v1` | Base URL for the `local` OpenAI-compatible provider. |
| `opencode_free_enabled` | global | `true` \| `false` | `false` | Off by default. `true` makes the keyless OpenCode Free models available in the picker. No account and no API key: requests go anonymously to a third-party relay, and several of those free models may train on what they receive. Turn it on only if the user asked for free models and accepts that. |
| `notifications_filter` | global | `all` \| `unread` | `all` | Which notifications the bell shows. |
| `notification_toasts` | global | `true` \| `false` | `true` | Whether a notification pops an in-app toast on a device the user is looking at. `false` silences the pop-up: the notification still counts on the bell badge and waits in the Notifications panel, and no OS push arrives in its place, because a device that is present never gets one. Unlike `push_notifications` this covers every device, so one write silences them all. |
| `mobile_header_sticky` | global | `true` \| `false` | `true` | Keep the mobile header always visible. |
| `external_link_target` | global | `safari` \| `ask` \| `in-app` | `safari` | Where an external http(s) link goes when tapped in an **installed iOS PWA**. No effect on desktop, Android, or a normal Safari tab, which all open a new tab. `safari` hands it to the Safari app; `ask` opens the OS share sheet so iOS offers every installed browser, including the user's real default; `in-app` keeps it in the PWA's in-app web view (no address bar, no shared Safari session). |
| `welcome_suggestions_dismissed` | global | `true` \| `false` | `false` | Hide the new-workspace welcome message. Set `false` to SHOW it again. |
| `self_curated_context_mode` | global | `true` \| `false` | `false` | **Experimental**, and off unless the user asked for it. `true` puts a chat or trigger thread in *self-curated context mode*. Tool results are then swept away in batches: every ten rounds the sweep takes everything more than five rounds old, and the call that made each one goes with it. Nothing stands in their place, and doing nothing holds a result until then. The agent keeps its picture of the job in a *working understanding* it writes as ordinary text in its own reply, and holds one item longer by naming its `evt-<hex>` address under a `[KEEP OPEN]` heading there. A *context panel* at the tail of every round states how full the prompt is, what each item costs and how long it has left. Everything else rides as it always did, except that the previous turn's tool calls are not re-sent, the conversation summariser does not run, and `todo_write` is withdrawn because the checklist moved into the same block. The cost is a re-fetch: a result the agent needed and did not write down takes a round to read back. Coding-agent threads are unaffected. |
| `self_curated_context_expire_after_rounds` | global | 1-1000 | `5` | How old a tool result has to be before a sweep may take it, in rounds. Only read when `self_curated_context_mode` is `true`. With the default sweep interval an item lives 6 to 15 rounds and averages ten. Provisional: the prompt and the panel both quote whatever is in force. |
| `self_curated_context_sweep_every_rounds` | global | 1-1000 | `10` | How often the sweep runs, in rounds. Only read when `self_curated_context_mode` is `true`. Removing a pair from the middle of the request invalidates every cached byte after it, so the pass is scheduled rather than run every round: nine rounds in ten are pure appends. Setting it to `1` restores a per-round drop and pays that cost every round. |
| `coding_agent_default` | global | `claude-code` \| `codex` | `claude-code` | Default coding agent the compose picker pre-selects. |
| `coding_agent_claude_path` | global | absolute path | (auto-detected) | Path to the `claude` CLI for Claude Code threads. Unset = auto-detect (`~/.local/bin`, `~/.claude/local`, Homebrew, PATH). A set path that doesn't resolve fails the spawn naming this key — never a silent fallback. |
| `coding_agent_codex_path` | global | absolute path | (auto-detected) | Path to the `codex` CLI for Codex threads. Unset = auto-detect (`~/.local/bin`, Homebrew, PATH). A set path that doesn't resolve fails the spawn naming this key — never a silent fallback. |
| `coding_agent_claude_permission_mode` | global | `accept-edits` \| `auto` | `accept-edits` | Which of Claude Code's own permission modes coding-agent threads run in. `accept-edits` cards anything outside the session's working directories. `auto` lets Claude Code's safety classifier approve routine actions instead, reaching shapes no allowlist can (a `cd` combined with a redirect, a write, or git). Four costs: it ignores a bare `Bash` entry in `cc-allowed-tools`, it denies rather than cards when the classifier is unreachable, a denial streak falls back to prompting, and each gated call pays a classifier round-trip. Claude Code only; Codex ignores it. Applies to new sessions. |
| `backup_schedule` | global | 6-field cron (in the user's timezone) or `off` | `off` | Automatic backup schedule. E.g. `0 0 3 * * *` = daily 03:00, `0 0 3 * * 0` = weekly Sun 03:00, `0 0 */12 * * *` = every 12h. Fires in the user's `timezone`. Requires `backup_provider` set AND its account connected (see that key). |
| `backup_provider` | global | `google_drive` \| `dropbox` | (unset) | Cloud destination, independent of `backup_schedule`: it stays set with the schedule `off`, and the Backup page's provider dropdown both opens on it and writes it. Setting this connects NOTHING: the account is connected with `connect_oauth_account`, or by the user in **Settings → Accounts** (never on the Backup page, which has no account UI). Until then backups run and the upload fails. `get_backup_status` reports whether the account is connected. |
| `backup_retention` | global | number 1–50 | `5` | How many recent backups to keep; older ones are pruned after each successful backup. |
| `backup_reminder_dismissed` | global | empty \| an RFC 3339 instant \| `forever` | (unset) | Dismissal state of the app-shell banner shown while backup is off (no active `backup_schedule` with a `backup_provider`). Unset/empty = never dismissed, banner shows. An RFC 3339 instant = dismissed then, hidden for 30 days from it. `forever` = dismissed a second time, hidden for good. Set it to empty to bring the reminder back. The banner only ever shows while backup is off, so enabling a schedule hides it whatever this says. |
| `theme` | device | `light` \| `dark` \| `system` | `system` | Color theme for the calling device. The default `system` follows the OS light/dark setting; `light` and `dark` pin it. |
| `font-family` | device | `monospace` \| `system` \| `inter` \| `jetbrains-mono` \| `ibm-plex-mono` \| `fira-code` | `fira-code` | UI font for the calling device. The default `fira-code` is served by this engine, so it needs no internet; `inter` / `jetbrains-mono` / `ibm-plex-mono` are fetched from Google Fonts on first use, and `monospace` / `system` use the device's own fonts. Fira Code also enables programming ligatures, on code surfaces only (code blocks, inline code, diffs, file previews); prose and the prompt render literally, because Fira Code's contextual alternates re-space a typed `...` into what reads as two dots. |
| `ui-scale` | device | number 75–200 | `100` | UI scale percent for the calling device (snaps to 12.5 steps). |
| `push_notifications` | device | `enabled` \| `declined` | (unset) | Push notifications for the calling device. `enabled` triggers the OS/browser permission prompt. |

## Read-only / managed elsewhere

`get_preferences` also surfaces these so you can explain them, but
`set_preference` refuses them (it returns a hint pointing at the right Settings
surface):

- `command_guard`, `command_guard_judge`, `model_command_judge`,
  `reasoning_command_judge`: the command guard (safety gate over the agent's own
  bash/python) and the model selection its LLM judge runs on. Settings →
  Permissions only. You must not disable or weaken your own safety gate.
- `backup_last_run` — internal backup state (the last run's outcome), not a
  setting. The backup *schedule*, *provider*, and *retention* ARE settable (see
  the table above); use `get_backup_status` to read the current schedule, next /
  last run, and recent history.
- `max_tool_calls`: how many tool calls you may make in a single turn before
  the engine stops the turn with an `[ENGINE-LIMIT]` message. Counts individual
  calls, not replies, so three calls in one reply spend three of them.
  Defaults to `500`;
  the user may set any number (there is no ceiling, and a value below `1` is
  raised to `1`). Changed in Settings → Models → Chat & triggers, never via
  `set_preference`: this is the backstop over your own loop, so you must not
  raise your own limit. You still cannot observe your own tool-call count while
  a turn runs; the `[ENGINE-LIMIT]` prefix is the only signal the cap was hit.
- `keybindings` — Settings → Keyboard Shortcuts.
- `capture_context` — a debug-only toggle.
- `network_bind` — this workspace's engine network bind (`loopback` / `all` /
  a specific tailnet IP). A security setting changed in Settings → Access →
  Network access, never via `set_preference`. Takes effect on the next engine
  restart. (The machine-global gateway bind + the engine-inherit toggle live in
  `~/.lucidos/network.toml`, not here.)
- `engine_switch_dismissed_build`, `client_refresh_dismissed_build` — internal UI
  state, not settings. Each holds the build id the user deferred a "new version"
  toast for — the on-disk engine binary build id (the *Switch to new version*
  toast) and the served client build id (the *refresh to sync* toast),
  respectively. Workspace-global (`device_id IS NULL`) so a dismiss on one device
  defers the toast on every device; a genuinely newer build (a different id)
  re-surfaces it everywhere. Managed by the version-update toasts, never via
  `set_preference`.
- `release_notice_cursor`: internal UI state, not a setting. It holds the id of
  the last *release notice* this workspace answered, and everything after it in
  the authored order is still owed. Workspace-global (`device_id IS NULL`), so
  answering on one device settles it on every device. Written by the engine when
  the user answers a notice. A workspace with no threads is also stamped once at
  startup, so nothing lands over its first run. Never via `set_preference`:
  clearing it re-shows every notice the user has already read.
- `provider_enabled_vertex`, `provider_enabled_anthropic`,
  `provider_enabled_openai`, `provider_enabled_openrouter`,
  `provider_enabled_xai`, `provider_enabled_local`: the per-provider enable
  switches on Settings → Models → Providers. Absent means **on**, and only an
  explicit `false` switches a provider off. Off drops it from the model picker
  and from web search, with no restart. It leaves the stored key untouched, so
  the user can park a key and pick it up later. Never via `set_preference`: the
  provider you switch off may be the one answering this turn. Contrast
  `opencode_free_enabled` above, which IS settable, because turning a keyless
  free tier on cannot leave the workspace unable to answer.

> Keep this file in lockstep with `core/preference_catalog.rs` — a `cargo test`
> sync test fails if a catalog key is missing here (see
> `.claude/rules/system-knowhow.md`).
