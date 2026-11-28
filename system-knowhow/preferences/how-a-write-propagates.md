---
name: Preferences, How a write propagates
description: Part of Preferences. Its index lists the sections it covers.
---

One part of `system-knowhow/preferences`, which lists every part.

## How a write propagates

`preferences(action="set")` validates against the **preference catalog**
(`crates/lucidos-core/src/core/preference_catalog.rs`, the single source of
truth). It writes through the engine's one preference chokepoint, which emits the
persisted **`PreferencesChanged`** event (or `LanguageSet` / `TimezoneSet` for
locale). Open Lucidos pages live-apply on those events, with no reload, no
transient event and no restart.

## Who may write a key

The catalog marks each key agent-settable or not. A key that is not belongs
to the owner, who changes it in Settings. Examples are the command guard, the
tool-call cap, the network bind, the provider switches and the local model
host. Every path holds the agent to that:

- `preferences(action="set")` refuses it with the place to change it instead.
- `lucidos preferences set` and a raw `PUT` or `DELETE /api/v1/preferences`
  from a Lucidos-spawned process answer 403. The engine
  knows the process by the origin token it handed it.
- An app is refused the same keys, and a few more (`js-sdk.md`).

The same 403 covers confirming an email draft (`POST /api/v1/email/send`) and
revealing a credential or the backup key. Those are the user's own buttons.

**`chat_model` / `chat_reasoning_efforts` are the default for NEW threads only.**
(`trigger_model` is the same for intent triggers, applying from their next fire.)
A Lucidos Agent thread reuses the model and reasoning effort it last ran with
(*per-thread model memory*). So a running thread keeps its model and effort,
**including the thread you are in when you make the change**. These keys only
set the fallback for a brand-new thread's first message.

With no stored model, automatic selection checks the configured routes and known account availability.
When Vertex is the only configured backend, it tries Gemini 4 Neon and then Carbon first.
Otherwise it tries Opus 5.5, GPT-6.2 Sol, GPT-6.1 Sol, Sonnet, Neon and Carbon in that order.
Both orders end with Flash and a stable registry fallback.
Gemini 4 Neon defaults to high and also offers xhigh.

An automatic selection without a documented API effort uses medium, clamped to the selected route.
A stored model or an explicitly configured router default remains a pin, including when unavailable.
Resolving an automatic default never rewrites these preferences.

The effort is remembered per model. Resolution order for the model the turn
runs on: explicit per-request override → the thread's last tier on that model
→ that model's entry in `chat_reasoning_efforts` → its *default effort*. A
model with no default effort sends none. Switching a thread to another model
never carries the old model's tier.

To change a *started* thread's model or
effort, use its *rung control* in the compose bar. That records a per-thread
`ChatSettingsChanged` and never touches this account default. A pick made while
a turn runs waits: the running turn keeps its pair, and the next turn runs on
the pick.

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
per-device and override the global value on that device. `preferences(action="set")`
targets the calling device; you never pass a device id. The trap: a global
`theme-mode=dark` does nothing on a device with its own `theme-mode=light`
override. `preferences(action="get")` shows the per-device effective value and the global one.

## Background models

**Every background task runs on whichever configured provider serves its
model.** That covers titles, change summaries, image descriptions, memory,
query classification, conversation summaries, `find`, the command judge, the
read decision, the wake decision and the compactor.

- **Unset, a key resolves for the providers the user has.** Its own default
  runs if a configured provider serves it. Otherwise the first of
  `gpt-6-luna` and `gemini-3.1-flash-lite` that one serves runs, and with
  neither, the chat model. Most tasks default to `claude-haiku-5-5`, the
  command judge to `gemini-3.1-flash-lite`. The compactor has its own list
  (see `model_summary_compaction`).
- **Four tasks try a measured model before their default.** Fact extraction
  tries `gpt-6-luna` first. The conversation summary tries the compactor's
  three first. The read decision tries `gemini-3.8-flash`, then `gpt-6-luna`.
  The wake decision tries `gemini-3.8-flash` and `gpt-6.1-sol`, then its
  default `gpt-6-luna`. Each task has its own key, and none follows another.
- **The wake decision runs only on a measured model.** It skips the shared
  fallbacks and the chat model. With none of its three served, it asks nothing
  and wakes the parent.
- **A key the user set is never moved to another model.** If no configured
  provider serves it, those calls fail, and Settings says so on the row.
- **The one exception is a retired model.** A provider retired it, so an
  engine update moves every key, trigger pin and thread off it. They move to
  its *successor*, named on its row in `GET /api/v1/models`. A call that still
  names it is sent as the successor. Lucidos never leaves a retired id for the
  provider to reroute.
- **A retired model's successor runs only where a provider serves it.** A key
  names its model's whole line of versions. Where the successor is not served,
  the key runs the newest earlier version that is, never a different model. A
  task's default never steps back to an earlier version. The task's row in
  `GET /api/v1/models/background` carries `generation` when another version
  runs, and Settings names it.
- **`GET /api/v1/models/background` says which model each task runs on**, and
  why: the preference, the default, or the chat model.
- **Settings offers every model on every row**, with the task's recommended
  models first. Image description is the exception.
- **Unset, a task's reasoning tier binds its recommended models.** Title runs
  at `none` on `claude-haiku-5-5`, for example. Any other model runs at
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

- **A new workspace asks.** While `memory_module` is unset, the workspace runs
  Classic, and the first client to open it shows the *memory choice*. That
  modal shows what Tree costs a month and writes the answer. Closing it writes
  `classic`. A headless workspace with no client open stays on Classic.
- **Do not choose for the user.** Tree costs money, and writing `tree` with
  `preferences(action="set")` skips the cost the memory choice and Settings show. Even
  when the user asks for Tree, point them at Settings → System → Memory, which
  shows the cost before it starts.
- `tree` gives each turn two *memory views*. The workspace
  memory view holds summary lines over every thread turn and artifact write.
  The thread memory view holds this thread, recent entries verbatim. A loaded
  knowhow doc shows there as a pointer, since its body is in the loaded
  knowhow section. The agent gets the `recall` tool in
  place of Classic's `memory` tool, which edits a store a Tree turn never reads.

  `zoom` opens a view line down to the exact message, and `find` walks the tree
  for a topic. `search` finds exact words, and `date` says when a line's
  entries happened. Coding-agent sessions get the workspace view in their system prompt
  and the same four as `lucidos recall` commands.
- `classic` is the conversation summariser, *memory recall* and *memory
  search*. It is almost free: on long threads it makes a small summary call now and then. A workspace used before the memory
  choice existed stores `classic`, so it is never asked.
- Choosing `tree` starts the compactor, which summarises the workspace once in
  the background. **Turns stay on Classic until the workspace tree and the
  threads active in the last 7 days are built.** So setting it changes nothing
  a user sees at first. Older threads fill in after; until then a turn in one
  reads its raw messages. Switching back to `classic` pauses the compactor and
  keeps what it built. Choosing `tree` again catches up first, and turns stay
  on Classic until it has.
- **The backfill costs money, so Settings asks first.** The first time,
  tapping Tree opens a
  dialog with the compactor model and an estimate for this workspace. It shows
  calls and cost. It shows the time until turns use Tree ("Usable in") and the
  time until every tree is built ("Complete in"). It also shows the cost a day
  after that.

  Each figure is a
  range. Only its
  Start Tree button writes `tree`; Cancel spends nothing.

  The estimate is
  `GET /api/v1/memory/tree-backfill/estimate`. Once a backfill has stored
  summaries, tapping Tree again writes `tree` at once and resumes it.
- **Backfill calls run on OpenAI's Flex tier** when OpenAI serves the
  compactor model: half the price, a little slower. A call refused twice for
  Flex capacity runs on the standard tier. Summaries of live turns always use
  the standard tier.
- **A backfill pauses at a spending cap**, 1.5 times the estimated cost,
  which the dialog states. While it waits, threads it already summarized keep
  up with new messages, and the rest wait with it. The user gets a notification, and the page reads "Paused at the
  $X spending cap" with two buttons: Continue to a higher cap, or Switch to
  Classic. Continue is `POST /api/v1/memory/tree-backfill/cap`. Leave that
  choice to the user. A model with no price in the estimate gets no cap.
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
- **The spend shows under the bar** while a backfill runs, as "$12.40 of
  $30.00 cap".
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

