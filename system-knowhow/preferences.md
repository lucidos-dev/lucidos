---
name: Preferences
description: The preferences (Settings) the Lucidos Agent reads and changes with get_preferences / set_preference: theme mode, language, timezone, push, welcome message, chat model, reasoning effort, UI scale, font and more, with allowed values, defaults and global-vs-device scope.
---

# Preferences (Settings the agent can change)

Lucidos **Settings** spans several stores. This file covers the
**preferences** store: the key→value settings the *Lucidos Agent* reads and
writes with the grouped **`preferences`** tool (`action: get | set`). The CLI
mirrors it as `lucidos preferences get | set` (see `lucidos-cli.md`). Here,
`get_preferences` / `set_preference(key, value)` are shorthand for
`preferences(action="get")` / `preferences(action="set", key, value)`. The old
flat tool names still work as aliases.

- **`get` (`get_preferences`)**: lists every settable preference with its current
  value, allowed values, default, and scope (global or per-device). Call it when
  you are unsure of a key or a valid value, or suspect a per-device override.
- **`set` (`set_preference(key, value)`)**: changes one preference. `value` is
  always a string (`"true"`/`"false"` for booleans, `"125"` for numbers, the
  exact enum string otherwise).

The other Settings stores have their own tools, never `set_preference`:

| Want to change… | Use |
|---|---|
| A preference below | `set_preference` |
| Which models appear in the picker, or a model's routes, preferred provider or context window | `manage_models` |
| An API key / secret | `request_credential` (never put secrets in a preference) |
| A non-secret env var | `env_vars` (`action: set`) |
| A registered repo | `manage_repositories` |
| An MCP server | `setup_mcp_server` / `start_mcp_server` / … |
| Command-safety (the command guard) | not agent-settable: Settings → Permissions |

## How a write propagates

`set_preference` validates against the **preference catalog**
(`crates/lucidos-engine/src/core/preference_catalog.rs`, the single source of
truth). It writes through the engine's one preference chokepoint, which emits the
persisted **`PreferencesChanged`** event (or `LanguageSet` / `TimezoneSet` for
locale). Open Lucidos pages live-apply on those events, with no reload, no
transient event and no restart.

**`chat_model` / `chat_reasoning_efforts` are the default for NEW threads only.**
A Lucidos Agent thread reuses the model and reasoning effort it last ran with
(*per-thread model memory*). So a running thread keeps its model and effort,
**including the thread you are in when you make the change**. These keys only
set the fallback for a brand-new thread's first message.

The effort is remembered per model. Resolution order for the model the turn
runs on: explicit per-request override → the thread's last tier on that model
→ that model's entry in `chat_reasoning_efforts` → its *default effort*. A
model with no default effort sends none. Switching a thread to another model
never carries the old model's tier. To change a *running* thread's model or
effort, use its in-thread model picker in the compose bar. That writes a
per-thread value and never touches this account default.

`chat_reasoning_efforts` REPLACES THE WHOLE LIST. Read the current value first
and send it back with your edit applied, or the other models lose their tiers.

**The style library.** `response_styles` holds ONLY what the user changed. An
entry whose id is `concise`, `minimal` or `learning` overrides that shipped
style. A fresh kebab-case id adds a style, and removing an entry restores the
shipped text. `standard` is the off switch and is refused here.

The key REPLACES THE WHOLE ARRAY. Read the current value first and send it back
with your edit applied, or you delete every style the user wrote. Bounds are
refused, not trimmed: 40 characters of id, 40 of label, 1000 of instruction, 20
entries.

`GET /api/v1/response-styles` is the easier read. It returns the merged library.
Each row carries `source` (`builtin`, `overridden` or `user`) and the shipped
text you would override.

**When the user asks for shorter or longer answers, SET `response_style`.**
Saying it in chat lasts one thread; the setting makes it stick. A change applies
from their NEXT message: a turn builds its prompt once, at the start. It
reaches triggers too, and changes nothing about coding-agent sessions.

**Technical literacy is the response style's second part.** `response_style`
sets the shape of an answer; `technical_literacy` sets how technical the words
are. Any style works with any level, and the engine joins them in one prompt
section. A technical user who wants only the outcome is a technical level plus a
short style, not a lower level. It is a mandatory preference: the first chat
turn asks for it, and `not-set` records a decline. It reaches chat and triggers
from the next message, and a coding-agent session or voice call from its next start.

**When the user says how technical they are ("I'm not technical", "I'm a
developer"), SET `technical_literacy`.** Store only a level they stated or
picked. Never infer one from how they write: a guess colours every answer.

**Device scope.** Device-scoped keys (theme-mode, theme, theme-effects,
font-family, ui-scale, motion, autocorrect, push_notifications) are stored
per-device and override the global value on that device. `set_preference`
targets the calling device; you never pass a device id. The trap: a global
`theme-mode=dark` does nothing on a device with its own `theme-mode=light`
override. `get_preferences` shows the per-device effective value and the global one.

## Background models

**Every background task runs on whichever configured provider serves its
model.** That covers titles, change summaries, image descriptions, memory,
query classification, conversation summaries, `find`, the command judge and
the compactor.

- **Unset, a key resolves for the providers the user has.** Its own default
  runs if a configured provider serves it. Otherwise the first of
  `gemini-3-flash-preview`, `gpt-5.4-mini` and `claude-haiku-4-5` that one
  serves runs, and with none of those, the chat model. The compactor has its
  own list (see `model_summary_compaction`).
- **Two tasks try a measured model before their default.** Fact extraction
  tries `gpt-5.6-luna` first. The conversation summary tries the compactor's
  three first. Each memory task has its own key, and none follows another.
- **A key the user set is never moved to another model.** If no configured
  provider serves it, those calls fail, and Settings says so on the row.
- **`GET /api/v1/models/background` says which model each task runs on**, and
  why: the preference, the default, or the chat model.
- **Settings offers every model on every row**, with the task's recommended
  models first. Image description is the exception.
- **Unset, a task's reasoning tier binds its recommended models.** Title runs
  at `none` on `gemini-3-flash-preview`, for example. Any other model runs at
  its own *default effort*, and one with none sends no effort at all. The
  compactor keeps `low` on its three measured models.
- **Image description runs only on a model that reads images**, by the model's
  vision flag (`manage_models` `vision`, the image icon in Settings → Models).
  Its picker lists only those, and its default skips the rest. A set key that
  cannot read images is refused, not replaced: no description runs, and
  Settings says why on the row.

## The memory module

**`memory_module` picks how a turn gets its past** (ADR 0362). The user sets
it under Settings → System → Memory. While it is `tree`, Settings → Models →
Background tasks also shows the compactor's model, `model_summary_compaction`.

- `classic`, the default, is the conversation summariser, *memory recall* and
  *memory search*.
- `tree` gives each turn two *memory views*: a workspace memory view (summary
  lines over every thread turn and artifact write) and a thread memory view
  (this thread, recent entries verbatim). The agent gets the `recall` tool in
  place of Classic's `memory` tool, which edits a store a Tree turn never reads.
  `zoom` opens a view line down to the exact message, and `find` walks the tree
  for a topic. `search` finds exact words, and `date` says when a line's
  entries happened. Coding-agent sessions get the workspace view in their system prompt
  and the same four as `lucidos recall` commands.
- Choosing `tree` starts the compactor, which summarises the workspace once in
  the background. **Turns stay on Classic until the workspace tree and the
  threads active in the last 7 days are built.** So setting it changes nothing
  a user sees at first. Older threads fill in after; until then a turn in one
  reads its raw messages. Switching back to `classic` pauses the compactor and
  keeps what it built. Choosing `tree` again catches up first, and turns stay
  on Classic until it has.
- **The backfill costs money, so Settings asks first.** The first time,
  tapping Tree opens a
  dialog with the compactor model and an estimate for this workspace: calls,
  cost, the time until turns use Tree ("Usable in"), the time until every tree
  is built ("Complete in"), and the cost a day after that. Each figure is a
  range. Only its
  Start Tree button writes `tree`; Cancel spends nothing. The estimate is
  `GET /api/v1/memory/tree-backfill/estimate`. Once a backfill has stored
  summaries, tapping Tree again writes `tree` at once and resumes it.
- **Writing `tree` yourself skips that dialog and starts spending at once.**
  When a user asks for Tree, send them to Settings → System → Memory. There
  they read the estimate and press Start, rather than you setting it.
- **While Tree is chosen, the page shows a progress bar** in place of the
  Classic memory list. It reads "Building memory, 42%" while the backfill
  runs. A line under it counts the trees and the summaries of the trees under
  way. Each thread is a tree, and so is the workspace. The summaries move the
  bar while one long tree holds the tree count.
- **The bar's other labels.** "A summary failed, retrying" means a tree goes
  back on the queue shortly. "Waiting for a background model" means no
  compactor model is usable, so nothing moves until one is picked.
- **"Ready"** means turns now use Tree.
  "Ready. Filling in older threads, 62%" means turns use Tree while older
  threads are still summarised. `GET /api/v1/memory/tree-backfill` serves the
  same state.
- **Under the bar, the page browses the summary trees.** Workspace shows the
  workspace tree's top lines, and Threads lists the threads with trees. Each
  line opens one level at a time, down to the exact message, through the same
  `zoom` the agent uses.
  `GET /api/v1/memory/tree` serves a tree's top, and
  `GET /api/v1/memory/tree/threads` the thread list.
- The view sizes are bytes, set per surface with the `workspace_view_bytes_*`
  keys, `thread_view_bytes` and `memory_view_model_caps` below.

## Settable preferences

| Key | Scope | Allowed values | Default | What it does |
|---|---|---|---|---|
| `language` | global | text | (detected from conversation) | Language for responses + session summaries (e.g. "English", "Norwegian"). A *voice session* speaks it. On a Realtime talker it also pins the transcriber to the matching ISO-639-1 code; a Live one has no such setting (ADR 0198). |
| `timezone` | global | IANA timezone | (unset) | Timezone for triggers + time display (e.g. "Europe/Oslo"). Set before creating triggers. |
| `chat_model` | global | a model id from the registry | `claude-opus-5` | Default chat model for NEW threads (see "How a write propagates"). `manage_models(action='list')` shows the options. |
| `chat_reasoning_efforts` | global | `model=tier` pairs, comma separated | (unset: each model's default effort) | Thinking budget for NEW threads per chat model, for example `claude-opus-5-5=high`. A model not listed runs at its default effort (`manage_models` `list` shows it). Clamped per model. |
| `response_style` | global | the id of a style in the library | `standard` | The shape of a chat or trigger answer: how much comes back, and what it is for. `standard` adds nothing. Shipped beside it: `concise`, `minimal` (the outcome, not the process) and `learning` (explains the why as it goes). The user can edit those and add their own, so the set is OPEN: read `response_styles` or `GET /api/v1/response-styles` for the ids here. An undefined id falls back to `standard`. |
| `response_styles` | global | JSON array of `{id, label, instruction}` | (unset: the shipped styles only) | The user's own styles, and their edits to the shipped ones. See "The style library" above. |
| `technical_literacy` | global | `not-set` \| `non-technical` \| `technical` \| `developer` | `not-set` (adds nothing; set it to clear a level) | How technical the words are in every answer, from plain words with no jargon (`non-technical`, shown as Keep it plain) to engineering terms with no explanation (`developer`). It never sets how much to say. An old `everyday` value reads as `non-technical`. Reaches chat, triggers, coding-agent sessions and voice calls. See "Technical literacy" above. |
| `image_model` | global | `auto` \| `imagen-4` \| `gpt-image-1` \| `gpt-image-1.5` \| `gpt-image-2` | `auto` | Model used by `generate_image`. |
| `model_title` | global | a model id | `gemini-3-flash-preview`, if reachable (see "Background models") | Background model for thread titles. |
| `reasoning_title` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for title generation. Naming a thread needs none. |
| `model_change_summary` | global | a model id | (the `model_title` model) | Background model that writes a *change summary*: one line saying what a coding-agent change of several commits does. It heads the change card, the change toasts and the Changes panel. A single-commit change makes no call: its commit subject is the line. Inherits `model_title` while unset. |
| `reasoning_change_summary` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | (the `reasoning_title` value, else `none`) | Thinking budget for the change summary, which condenses commit subjects into one line. Inherits `reasoning_title` while unset. |
| `model_summary_compaction` | global | a model id | (resolved from the configured providers) | Background model for the compactor, which writes the summary lines of the Tree memory module. It runs only while `memory_module` is `tree`. While unset, the engine picks the first of `gpt-6.1-sol`, `gemini-3.8-flash` and `claude-sonnet-5-5` that a configured provider serves, and falls back to the chat model. `GET /api/v1/models/background` says which model that is. Settings recommends those three first. |
| `reasoning_summary_compaction` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | (`low` on the default models, else the model's default effort) | Thinking budget for the compactor. While unset it runs its three default models at `low`, the tier their comparison measured, and any other model at its own default effort. |
| `model_memory_find` | global | a model id | `gemini-3-flash-preview`, if reachable (see "Background models") | Background model the recall tool's `find` asks its yes/no questions on, when `judgment_memory_find` leaves it on chat. Each call judges about 40 summary lines. |
| `reasoning_memory_find` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for `find`, which answers short yes/no questions. |
| `memory_module` | global | `classic` \| `tree` | `classic` | How a turn gets its past. See "The memory module" above. Applies from the next turn. |
| `workspace_view_bytes_home` | global | number 0–262144 | `65536` | Bytes of workspace memory view a home thread turn reads, under the `tree` module. `0` turns the view off there. |
| `workspace_view_bytes_chat` | global | number 0–262144 | `16384` | The same for a chat thread turn. |
| `workspace_view_bytes_trigger` | global | number 0–262144 | `16384` | The same for a trigger run. Trigger runs are frequent, so this size costs most. |
| `workspace_view_bytes_coding_agent` | global | number 0–262144 | `0` | The same for a coding-agent session, in its system prompt at session start. Default off, so a coding agent starts with no memory, like Classic. Set a positive value to opt in. |
| `thread_view_bytes` | global | number 0–262144 | `65536` | The most bytes of a thread's own past a turn reads, under the `tree` module. Recent entries read verbatim, older ones as summary lines. |
| `memory_view_model_caps` | global | `model=bytes` pairs, comma separated | (unset) | Caps both memory views for named models, for example `claude-haiku-4-5=8192`. A model id matches with or without its `[1m]` suffix. |
| `model_image_description` | global | a model id | `gemini-3-flash-preview`, if reachable (see "Background models") | Background model that describes uploaded images. It must have the vision flag, or no description runs. |
| `reasoning_image_description` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for describing an image. Captioning is perception, so the default spends nothing. |
| `model_memory` | global | a model id | `gemini-3-flash-preview`, if reachable, after `gpt-5.6-luna` (see "Background models") | Background model that extracts facts from a turn for long-term memory. While unset, `gpt-5.6-luna` runs when a configured provider serves it. It binds extraction only. |
| `reasoning_memory` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `low` | Thinking budget for fact extraction, which returns short JSON on every turn. At `none`, GPT-5.6 Luna and GPT-5.4 mini missed facts. |
| `model_conversation_summary` | global | a model id | `gemini-3-flash-preview`, if reachable, after the compactor's three (see "Background models") | Background model that writes a thread's *conversation summary*: the paragraph standing in for its older assistant turns. While unset, the first of `gpt-6.1-sol`, `gemini-3.8-flash` and `claude-sonnet-5-5` that a configured provider serves runs. Its input can be 80k tokens, far larger than any other background call. |
| `reasoning_conversation_summary` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `low` | Thinking budget for the conversation summary. Raising it is unlikely to help: output length does not track it, and the summariser's failures are calls that never complete. |
| `model_query_classification` | global | a model id | `gemini-3-flash-preview`, if reachable (see "Background models") | Background model that decides what a turn needs retrieved: long-term memory, the file list, credentials. Settings offers the System One models (Jev, Clef, Clef-flash, a custom endpoint) in the same control, which write `judgment_query_classification` instead. |
| `reasoning_query_classification` | global | `none` \| `low` \| `medium` \| `high` \| `xhigh` \| `max` | `none` | Thinking budget for query classification, which answers three yes/no questions before every turn. |
| `voice_enabled` | global | `true` \| `false` | `false` | Experimental. `true` puts a call control in the *home thread*'s composer and lets `/api/v1/voice` accept a socket there, so Home can be spoken to. A call runs on the home thread alone, which every workspace has. It rents a speech-to-speech talker and needs the OpenAI provider configured. Every spoken utterance starts an ordinary agent turn, so a short call can cost several. With this off, nothing voice-shaped is reachable and the voice keys below do nothing. |
| `model_voice_talker` | global | a model id | `gpt-realtime-2.1` | Speech-to-speech model a *voice session* speaks through; the id also picks the call's protocol. It holds the conversation only, so every action goes through the ordinary agent. Settings offers the realtime family newest first: `gpt-realtime-2.1`, `gpt-realtime-2.1-mini`, `gpt-realtime-2` and `gpt-realtime-1.5`. These bill by the token, and the caller can settle a card or ring off out loud. `gpt-live-1` follows: it takes an interruption better, holds no tools, and bills by the minute. A Live caller answers a question out loud, and taps for a permission or to ring off. |
| `model_voice_transcriber` | global | a model id | `gpt-4o-mini-transcribe` | Model turning the caller's speech into text in a realtime *voice session*. A `gpt-live-1` talker transcribes the caller itself and ignores this key. The second and last model in the voice loop: nothing translates or summarises, and `language` sets its pin. Settings offers six ids. The two built for a live microphone lead and stream as the caller speaks: `gpt-live-transcribe` and `gpt-realtime-whisper`. The engine sends `gpt-live-transcribe`'s language pin as `languages`, an array; every other model reads the singular `language`. The rest transcribe a turn once committed: `gpt-transcribe`, `gpt-4o-mini-transcribe`, `gpt-4o-transcribe` and `whisper-1`. |
| `voice_talker_voice` | global | a provider voice name | `marin` | The voice a call is spoken in. Not a model and not the language: `language` decides what is spoken, this decides who speaks it. A name the provider does not know refuses the call, so pick one the talker model offers. |
| `voice_resident_sections` | global | comma-separated section ids | `who-and-where,this-thread,workspace-shape` | What a *voice session* loads at the start of a call. The talker cannot look anything up, so this block is all voice answers with no wait; everything else waits for the agent. `who-and-where`: the workspace name, timezone and local time. `this-thread`: the title, the recent turns, and any question the thread is parked on. `workspace-shape`: the names of apps, triggers and unread notifications, plus two thread lines (what Lucidos was working on as the call opened, and what is stopped waiting on an answer). An unknown id is ignored with a log line. A stored row means exactly what it lists; only an absent row means the three above. Turning every section off writes an empty value, which `set_preference` rejects. That state is Settings-only, through the toggles in Settings > Models > Voice. |
| `vertex_region` | global | text | `europe-west1` | Google Vertex AI region. |
| `opencode_free_enabled` | global | `true` \| `false` | `false` | `true` makes the keyless OpenCode Free models available in the picker. No account and no API key: requests go anonymously to a third-party relay, and several of those models may train on what they receive. Turn it on only if the user asked for free models and accepts that. |
| `proxy_timeout_secs` | global | number 1–600 | `30` | Seconds the engine proxy waits on one upstream request before it answers 504. Covers every proxied call: `lucidos proxy`, `lucidos.proxy` in an app, the `proxy_request` tool, and the builtin model routes such as `vertex` and `openai`. A streamed reply counts in full, because the proxy reads the whole body first. Raise it when a long model call through the proxy times out. An `apis.json` entry's own `timeout_secs` wins for that entry. Every write path refuses a value outside the range. |
| `notifications_filter` | global | `all` \| `unread` | `all` | Which notifications the bell shows. |
| `notification_toasts` | global | `true` \| `false` | `true` | Whether a notification pops an in-app toast on a device the user is looking at. `false` silences the pop-up only: the notification still counts on the bell badge and waits in the Notifications panel. No OS push replaces it, because a present device never gets one. Unlike `push_notifications`, one write covers every device. |
| `mobile_dynamic_bars` | global | `true` \| `false` | `false` | *Dynamic bars* on a phone. `true` glides the header, the thread title and the prompt away while scrolling down a thread, and back on scroll up or a pane swipe. `false` keeps them always visible. |
| `automatic_widgets` | global | `true` \| `false` | `true` | Whether you may choose a *widget* on your own: a small interactive answer shown inline in the thread as a *widget card* (`create_app` with `kind="widget"`). `false` stops only your own choice, and a widget the user asks for is still made. Set it to `false` when the user tells you to stop making widgets. |
| `external_link_target` | global | `safari` \| `ask` \| `in-app` | `safari` | Where an external http(s) link goes when tapped in an **installed iOS PWA**. Desktop, Android and a normal Safari tab always open a new tab. `safari` hands it to the Safari app. `ask` opens the OS share sheet, so iOS offers every installed browser, including the user's real default. `in-app` keeps it in the PWA's in-app web view (no address bar, no shared Safari session). |
| `welcome_suggestions_dismissed` | global | `true` \| `false` | `false` | Hide the new-workspace welcome message. Set `false` to SHOW it again. |
| `self_curated_context_mode` | global | `true` \| `false` | `false` | **Experimental**: set it only if the user asked. `true` puts a chat or trigger thread in *self-curated context mode*. A sweep then removes tool results in batches: every ten rounds it takes everything more than five rounds old, with the call that made each one. Nothing stands in their place. The agent keeps its picture of the job in a *working understanding*, written as ordinary text in its own reply. It holds one item longer by naming its `evt-<hex>` address under a `[KEEP OPEN]` heading there. A *context panel* at the tail of every round states how full the prompt is, what each item costs and how long it has left. Three other changes: the previous turn's tool calls are not re-sent, the conversation summariser does not run, and `todo_write` is withdrawn (the checklist moved into the same block). The cost is a re-fetch: a result the agent needed and did not write down takes a round to read back. Coding-agent threads are unaffected. |
| `self_curated_context_expire_after_rounds` | global | number 1–1000 | `5` | How many rounds old a tool result must be before a sweep may take it. Only read when `self_curated_context_mode` is `true`. With the default sweep interval an item lives 6 to 15 rounds, ten on average. Provisional: the prompt and the panel both quote the value in force. |
| `self_curated_context_sweep_every_rounds` | global | number 1–1000 | `10` | How often the sweep runs, in rounds. Only read when `self_curated_context_mode` is `true`. Removing a pair from the middle of the request invalidates every cached byte after it, so the sweep is scheduled: nine rounds in ten are pure appends. `1` drops every round and pays that cost every round. |
| `workspace_prompt_footprint_section_ceiling` | global | number 100–1000000 | `6000` | Chars one section of the *workspace prompt footprint* may take before the *workspace audit* flags it. Nothing is cut: the prompt still carries the whole section. Change it only when the user asks, never to clear an audit finding. |
| `workspace_prompt_footprint_total_ceiling` | global | number 100–1000000 | `20000` | Chars the whole *workspace prompt footprint* may take before the *workspace audit* flags it. Nothing is cut. Change it only when the user asks, never to clear an audit finding. |
| `workspace_prompt_footprint_unused_days` | global | number 1–3650 | `60` | Days without use after which the *workspace prompt footprint* calls an app or reusable widget unused, and a knowhow doc not loaded by name. An item younger than this is not judged. Change it only when the user asks. |
| `coding_agent_default` | global | `claude-code` \| `codex` | `claude-code` | Default coding agent the compose picker pre-selects. |
| `coding_agent_claude_path` | global | absolute path | (auto-detected) | Path to the `claude` CLI for Claude Code threads. Unset = auto-detect (`~/.local/bin`, `~/.claude/local`, Homebrew, PATH). A set path that does not resolve fails the spawn and names this key; it never falls back silently. |
| `coding_agent_codex_path` | global | absolute path | (auto-detected) | Path to the `codex` CLI for Codex threads. Unset = auto-detect (`~/.local/bin`, Homebrew, PATH). A set path that does not resolve fails the spawn and names this key; it never falls back silently. |
| `coding_agent_claude_permission_mode` | global | `accept-edits` \| `auto` | `accept-edits` | Which of Claude Code's own permission modes coding-agent threads run in. `accept-edits` cards anything outside the session's working directories. `auto` lets Claude Code's safety classifier approve routine actions, reaching shapes no allowlist can (a `cd` combined with a redirect, a write, or git). Four costs: it ignores a bare `Bash` entry in `cc-allowed-tools`, it denies rather than cards when the classifier is unreachable, a denial streak falls back to prompting, and each gated call pays a classifier round-trip. Claude Code only; Codex ignores it. Applies to new sessions. |
| `backup_schedule` | global | 6-field cron (in the user's timezone) or `off` | `off` | Automatic backup schedule. E.g. `0 0 3 * * *` = daily 03:00, `0 0 3 * * 0` = weekly Sun 03:00, `0 0 */12 * * *` = every 12h. Fires in the user's `timezone`. Requires `backup_provider` set AND its account connected (see that key). |
| `backup_provider` | global | `google_drive` \| `dropbox` | (unset) | Cloud destination, independent of `backup_schedule`: it stays set with the schedule `off`. The Backup page's provider dropdown opens on it and writes it. Setting this connects NOTHING. Connect the account with `connect_oauth_account`, or the user does it in **Settings → Accounts** (the Backup page has no account UI). Until then backups run and the upload fails. `get_backup_status` reports whether the account is connected. |
| `backup_retention` | global | number 1–1000 | `5` | How many recent backups to keep; older ones are pruned after each successful backup. |
| `backup_reminder_dismissed` | global | empty \| an RFC 3339 instant \| `forever` | (unset) | Dismissal state of the app-shell banner shown while backup is off (no active `backup_schedule` with a `backup_provider`). Unset/empty = never dismissed, banner shows. An RFC 3339 instant = dismissed then, hidden for 30 days. `forever` = dismissed a second time, hidden for good. Set it to empty to bring the reminder back. Enabling a schedule hides the banner whatever this says. |
| `theme-mode` | device | `light` \| `dark` \| `system` | `system` | Light or dark for the calling device. `system` follows the OS; `light` and `dark` pin it. |
| `font-family` | device | `theme` \| `system` \| `geist` \| `atkinson-hyperlegible-next` \| `inter` \| `roboto` \| `open-sans` \| `manrope` \| `source-serif-4` \| `lora` \| `literata` \| `fira-code` \| `monospace` \| `geist-mono` \| `atkinson-hyperlegible-mono` \| `jetbrains-mono` \| `ibm-plex-mono` \| `source-code-pro` \| `commit-mono` \| `cascadia-code` \| `vt323` \| `ws-<slug>` | `theme` | UI font for the calling device. The default `theme` ("Follow the theme" in Settings) paints the font the active theme suggests, else `fira-code`. Any other value is the user's own pick and wins over the theme. `GET /api/v1/fonts` lists every font with its kind and group (sans, serif, mono). Every font is bundled, on the device, or a workspace font (`ws-<slug>`, installed under `data/fonts/`, see `system-knowhow/workspace-fonts.md`), so none needs the internet. Fira Code, JetBrains Mono and Cascadia Code enable programming ligatures on code surfaces only (code blocks, inline code, diffs, file previews); prose and the prompt render literally, because contextual alternates re-space a typed `...` into what reads as two dots. |
| `ui-scale` | device | number 75–200 | `100` | UI scale percent for the calling device (snaps to 12.5 steps). |
| `motion` | device | `system` \| `reduce` \| `full` | `system` | Whether animations are reduced on the calling device. `system` follows the OS reduce-motion switch. `reduce` calms the app whatever the OS says: no slides, pulses or spinners, and transitions become instant. `full` keeps every animation even when the OS asks to reduce. Found under **Settings → Appearance → Motion**. Offer `reduce` to a user who finds the app busy or reports motion discomfort. Apps read it as `data-motion` on `<html>`. |
| `theme-effects` | device | `system` \| `reduce` \| `full` | `system` | Whether the shadows, filters and scanlines a theme puts on its parts show on the calling device. `reduce` drops every part `text-shadow`, `box-shadow` and `filter` and the screen's scanlines. It keeps part colours, letter-spacing, the caret shape and borders. `system` drops them when the OS asks for more contrast or less transparency; it ignores battery state. `full` always shows them. Found under **Settings → Appearance → Theme → Effects**. Offer `reduce` to a user who finds a theme's glow hard to read or wants to save battery. Apps read it as `data-theme-effects` on `<html>`. See the `themes` knowhow. |
| `theme` | device | a theme id | `lucidos` | The theme on the calling device: a named set of colours, header and focus styles. A built-in id (listed in `themes.md`) or a workspace theme at `data/themes/<id>.json`. It works with `theme-mode`: a theme styles light, dark or both. An unknown id shows the default. Found under **Settings → Appearance → Theme**. To make a theme, load the `themes` knowhow. |
| `autocorrect` | device | `true` \| `false` | `true` | Whether text fields autocorrect as the user types on the calling device. While iOS autocorrect holds a correction, it can swallow the tap on Send or Submit until the keyboard closes. If an iPhone or iPad user reports that, closing the keyboard and tapping again gets through. Offer `false` if it keeps happening. Spell-check underlines and sentence capitals stay either way. Found under **Settings → System → Debugging**, on iPhone and iPad only. An app's own text fields follow it too, through `sdk.js`. |
| `push_notifications` | device | `enabled` \| `declined` | (unset) | Push notifications for the calling device. `enabled` triggers the OS/browser permission prompt. |

## Read-only / managed elsewhere

`get_preferences` also lists the settings among these so you can explain them,
and `set_preference` refuses every one with a hint naming the right surface:

- `command_guard`, `command_guard_judge`, `model_command_judge`,
  `reasoning_command_judge`: the command guard (safety gate over the agent's own
  bash/python) and the model its LLM judge runs on. Settings → Permissions only.
  You must not disable or weaken your own safety gate.
- `judgment_command_guard`, `judgment_query_classification`: which backend
  answers each judgment: `chat`, or a System One model (`jev`, `clef`,
  `clef-flash`, `custom`). Both ask typed questions and decide the outcome in
  code. `chat`, the default, asks the surface's own chat model. A System One
  model answers faster and sends what it judges to that vendor, and the chat
  model takes over if its call fails. Neither key has its own control: **the
  System One models are rows** in that surface's model dropdown. Choosing one
  writes its id; choosing any other model writes `chat`.
  - The first dropdown is the Judge model row in Settings → Permissions (Command
    safety). The second is the Query classification row in Settings → Models
    (Background tasks).
  - A System One row appears only once its provider is set up in Settings →
    Models (Providers), and each sits under that provider's switch below.
- `judgment_memory_find`: the same choice for the recall tool's `find`. It has
  no control yet, so `find` runs on chat (`model_memory_find`) unless the user
  sets it. Picking a System One model there would send memory lines to that
  vendor, so only the user decides.
- `backup_last_run`: internal backup state (the last run's outcome), not a
  setting. The backup *schedule*, *provider* and *retention* ARE settable (see
  the table above). `get_backup_status` reads the current schedule, next and
  last run, and recent history.
- `max_tool_calls`: how many tool calls you may make in one turn before the
  engine stops it with an `[ENGINE-LIMIT]` message. It counts calls, not
  replies: three calls in one reply spend three. Default `500`. The user may set
  any number of at least `1` (a value below `1` is raised to `1`) in Settings →
  Models → Chat & triggers. Never via `set_preference`: it is the backstop over
  your own loop, so never raise it. The `[ENGINE-LIMIT]` prefix is your only signal the cap was hit.
- `keybindings`: Settings → Keyboard Shortcuts.
- `capture_context`: a debug-only toggle.
- `voice_input_device`: which microphone a call opens, per device. The value is
  a browser's own opaque handle, meaningless in another browser or on another
  machine. The user picks it by holding the call control in the composer. You
  cannot know what the ids stand for, so never write one.
- `network_bind`: this workspace's engine network bind (`loopback` / `all` / a
  specific tailnet IP). A security setting, changed in Settings → Access →
  Network access, never via `set_preference`. Takes effect on the next engine
  restart. The machine-global gateway bind and the engine-inherit toggle live in
  `~/.lucidos/network.toml`, not here.
- `engine_switch_dismissed_build`, `client_refresh_dismissed_build`: internal UI
  state. Each holds the build id the user deferred a "new version" toast for.
  The first holds the on-disk engine binary's id (the *Switch to new version*
  toast). The second holds the served client's id (the *refresh to sync*
  toast). Workspace-global (`device_id IS NULL`): a dismiss on one device defers
  the toast everywhere, until a newer build (a different id) re-surfaces it.
  The version-update toasts manage them, never `set_preference`.
- `release_notice_cursor`: internal UI state. It holds the id of the last
  *release notice* this workspace answered; everything after it in the authored
  order is still owed. Workspace-global (`device_id IS NULL`), so answering on
  one device settles it everywhere. The engine writes it when the user answers a
  notice. It also stamps a workspace with no threads once at startup, so nothing
  lands over its first run. Never via `set_preference`: clearing it re-shows
  every notice the user already read.
- `vapid_keys`, `backfill_trigger_id_from_events_done`,
  `backfill_trigger_id_v5_to_config_id_done`,
  `backfill_repo_names_from_changes_done`,
  `backfill_cc_repo_id_to_deterministic_done`: engine bookkeeping, never shown
  by `get_preferences`. The first is the workspace's Web Push signing keypair;
  the rest are one-shot migration markers.
- `provider_enabled_vertex`, `provider_enabled_anthropic`,
  `provider_enabled_openai`, `provider_enabled_openrouter`,
  `provider_enabled_xai`, `provider_enabled_local`: the per-provider switches on
  Settings → Models → Providers. Absent means **on**; only an explicit `false`
  switches a provider off. Off drops it from the model picker and from web
  search, with no restart, and leaves the stored key untouched for later. Never
  via `set_preference`: the provider you switch off may be answering this turn.
  `opencode_free_enabled` above IS settable, because turning a keyless free tier
  on cannot leave the workspace unable to answer.
- `local_base_url`: the base URL of the `local` OpenAI-compatible provider,
  `http://localhost:11434/v1` by default. The user sets it in Settings → Models
  → Providers. Neither you (`set_preference`) nor an app may set it: that host
  reads every prompt and writes the replies you run as tool calls. The host must
  be loopback, a private or tailnet address, or a `.local` or `.home.arpa` name.
  Any other host is refused on save, and one stored earlier is ignored, so the
  local provider drops out. The saved `local` key goes only where its credential
  scope covers this URL.
- `provider_enabled_typesafe`, `provider_enabled_cloudflare_workers_ai`,
  `provider_enabled_system_one_custom`: the same switch for the three System
  One providers, on the same page: TypeSafe's Jev, Cloudflare's Clef and
  Clef-flash, and a custom endpoint. They keep the absent-means-**on** rule and
  the stored-key promise. Each is the master switch above every `judgment_*` key
  that picked one of its models: off, those run on their chat model whatever the
  key says. A System One model holds no conversation, so it is in no chat model
  picker and no `configured_providers` list. It is offered only in the judgment
  rows (ADR 0224). Never via `set_preference`, for a different reason:
  switching one off returns the command guard's backend to chat, which is not
  your decision.
- `system_one_custom_url`, `system_one_custom_model`: where the custom System
  One endpoint lives and which model it is asked for, set on its row in Settings
  → Models → Providers. That host receives every state a judgment sends it and
  can answer the command guard, so you never set either.

> Keep this file in lockstep with `core/preference_catalog.rs`: a `cargo test`
> sync test fails if a catalog key is missing here, or if a default stated here
> differs from the catalog's (see `.claude/rules/system-knowhow.md`).
