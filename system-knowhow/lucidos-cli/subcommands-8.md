---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos memory stats | entries [...] | search --q <Q> [--limit N] | source [--source-id UUID] [--source-type T] [--path P] [--commit C] | correct-by-id --id <UUID> [--correction <TEXT>]`

Read long-term memory with four subcommands:

- `stats` (index counts).
- `entries` (paginated, with importance and source).
- `search` (rank entries against a question).
- `source` (the originating event or artifact for one memory, plus the entries
  derived from it).

Those four only read.

`correct-by-id` deletes one entry, and stores `--correction` in its place when
given. It emits `MemoryCorrected`, which every memory rebuild replays, so the
deleted fact stays deleted. A SQL write to `memory_entries` is refused, and a
rebuild would have undone it anyway.

This is the long-term memory store behind *memory recall* and *memory search*.
It keeps filling under either *memory module*, and these commands read it under
both. Only a Classic turn reads it. A Tree turn gets its past from summary trees
instead, which `lucidos recall` below opens. So the agent's `memory` tool is
offered on Classic only, and on Tree the `recall` tool takes its place.

```bash
$ lucidos memory stats
$ lucidos memory entries --limit 20 --importance high,critical
$ lucidos memory search --q "launch outcome" --limit 5
$ lucidos memory source --source-id <uuid>
$ lucidos memory correct-by-id --id <uuid> --correction "The launch moved to May."
```

`search` ranks with the same `similarity * importance * recency` the chat
agent's injected memory block uses, so the CLI and the agent cannot disagree
about an order.

**`source` takes either id.** Pass the memory's own `[id: <uuid>]` (what a
memory bullet shows) or the source event's uuid; it resolves either. The
response carries the event's `thread_id`, which is what turns a fact back into
the conversation it came from.

Correcting memory is the chat agent's grouped `memory` tool (`correct` /
`correct_by_id`), not a CLI op. The agent has `search` and `source` too; what
it does not have is `stats` and `entries`, which are operator reads. On Tree
there is nothing to correct this way: the user's newer words are the
correction, and they hold over older lines.


### `lucidos recall zoom --id <ID> [--n N] [--thread T] | find --query <Q> [--limit N] [--thread T] | search --text <WORDS> [--limit N] | date --id <ID> [--thread T]`

The Tree *memory module*'s recall tools (`memory_module = tree`). A coding-agent
session on a Tree workspace finds the workspace memory view in its system
prompt; these open it. Each view line starts with its id: `w/12+4` for the
workspace tree, `<thread id>/12+4` for a thread's. A bare `12+4` names the
`--thread` thread, which defaults to `$LUCIDOS_THREAD_ID`, the session's own.

```bash
$ lucidos recall zoom --id w/0+64 --n 2
$ lucidos recall find --query "the parser rewrite" --limit 5
$ lucidos recall search --text "door colour"
$ lucidos recall date --id w/40+8
```

- `zoom` opens a line `--n` levels down (1-6, default 1). A workspace turn
  line opens into its thread's entries for that turn, and a thread leaf into
  the exact message. An artifact line reads the file at that commit.
- `find` walks the workspace tree, judging lines with the `judgment_memory_find`
  backend (chat by default) and opening the likeliest. Its calls' cost is
  filed on the `--thread` thread, the session's own by default.
- `search` matches every word in message text, newest first, and returns each
  message's thread id and its workspace line once built.
- `date` gives the first and last time among a line's entries.

The routes answer in any workspace, from whatever summary trees exist; a
Classic workspace has none. The Lucidos Agent reaches the same four through its
`recall` tool, offered only once a Tree workspace's trees are ready.

### `lucidos widgets list | size --app-id <ID> --size S [--thread-id T] [--params JSON] | fit --app-id <ID> [--thread-id T] | make-reusable --app-id <ID> | stop-reusing --app-id <ID> | dashboard [--dashboard D] | dashboard-add --app-id <ID> [--dashboard D] [--params JSON] [--label L] [--size S] | dashboard-remove --app-id <ID> [--dashboard D] [--params JSON] | arrange [--dashboard D] --tiles JSON`

*Reusable widgets*, a thread's widget card sizes, and the two *dashboards*. A
widget is an app folder whose app manifest says `"kind": "widget"`. A reply
embeds it, `![Name](app:<id>)`, and it shows there as an *inline widget*, never
in the apps list. A widget the user wants in view goes on a dashboard.
`size` and `fit` default `--thread-id` to `$LUCIDOS_THREAD_ID`, the session's
own thread.

```bash
$ lucidos widgets size --app-id fare-grid --size large
$ lucidos widgets size --app-id lucidos-sound-player \
    --params '{"clip":"artifacts/voices/marin.mp3"}' --size medium
$ lucidos widgets fit --app-id fare-grid
$ lucidos widgets make-reusable --app-id fare-grid
$ lucidos widgets list
$ lucidos widgets dashboard --dashboard desktop
$ lucidos widgets dashboard-add --app-id clock --dashboard phone
$ lucidos widgets dashboard-remove --app-id inbox --dashboard phone
$ lucidos widgets arrange --dashboard phone \
    --tiles '[{"app_id":"clock","size":"small"},{"app_id":"inbox","size":"medium"}]'
```

- `size`, `dashboard-add` and `dashboard-remove` name a *widget instance*: the
  widget plus `--params`, a JSON object of the names its manifest declares
  (ADR 0415). Two param sets are two instances, each with its own card size and
  tile. `--label` names the instance's tile; it is not part of the instance.
- There is no `show`, and no `pin`: a widget shows in a thread by an embed in
  a reply (ADR 0466), and a thread holds no pinned widgets.
- `size` sets the size of the instance's cards in the thread: `small`,
  `medium` or `large`, one its manifest declares. It writes one
  `InlineWidgetSized`, and the newest one wins, with no file or commit change.
  A card with none starts at the widget's default size. Another thread's
  widget must be reusable first, and set one only when the user named it.
  `POST /api/v1/widgets/size`, which answers `{"ok": true}`.
- `fit` runs the *widget fit check*, and writes nothing. An open Lucidos
  window renders every declared size on the desk tile and on a 360 px phone
  column. It uses the largest UI scale any device uses. It prints a
  `[WIDGET FIT]` note: a pass on one line, or each box that cuts content off,
  with its size, surface, frame px and the cut text. With no visible window,
  or no answer in time (6 s per size, plus 6 s), it prints `not checked` and
  the reason.

  A coding agent that writes widget files with its own tools runs it after
  each change. The thread names where the request rides. A calling device's
  window renders it when visible, else the most recently seen visible window.
  `POST /api/v1/widgets/fit`, which answers `{"note": "<text>"}`.
- `make-reusable` and `stop-reusing` flip the `reusable` flag in the app
  manifest and commit it. A widget that stops being reusable belongs to its
  origin thread again, so Delete (a thread) on that thread removes it. When
  that thread was deleted, `stop-reusing` is refused. It is refused too while
  a dashboard holds the widget, since deleting its thread would empty the tile.
- `list` names the reusable widgets.
- `--dashboard` is `phone` or `desktop`. Each dashboard has its own tiles, and
  a change to one never touches the other (ADR 0496). Over HTTP an omitted
  dashboard means the calling device's. The CLI sends no device, so name one.
- `dashboard` reads one dashboard: its tiles in order, each with its `kind`
  (`widget` or `app`), its size and the sizes it takes. `GET /api/v1/dashboard?dashboard=`.
- `dashboard-add` adds the instance as a tile, at `--size` when given (one
  its manifest declares), else at its widget's default size. It writes one
  `DashboardTileAdded`. A widget that belongs to its thread
  becomes reusable first, in the same action, and the answer says
  `"made_reusable": true`. `POST /api/v1/widgets/dashboard-add`.
- `dashboard-add` and `dashboard-remove` also take the id of an app that is
  not a widget. It becomes an *app shortcut*: a quarter-size tile that opens
  the app. It takes no `--params`, and an add with them is refused. `--label`
  renames it. `make-reusable` and `arrange` sizes still apply to widgets only,
  and an app shortcut only takes the size `quarter`.
- `dashboard-remove` takes the tile off that dashboard only, and writes one
  `DashboardTileRemoved`. `POST /api/v1/widgets/dashboard-remove`.
- `arrange` writes one `DashboardArranged` and prints the dashboard it gives.
  Each tile names an instance already on that dashboard, once, at a size its
  manifest declares; anything else is refused. A tile the list leaves out
  keeps its place after the named ones. To drop a tile, use
  `dashboard-remove`. `POST /api/v1/widgets/arrange`.

Make a widget with the chat agent's `create_app` (`kind="widget"`), or write
the folder yourself with `kind`, `origin_thread_id` and `reusable` in its
manifest. A widget a plugin ships is always reusable, so `stop-reusing`
refuses it. The chat agent has the grouped `widgets` LLM tool with the same
actions.

### `lucidos env-vars list | set --name <NAME> --value <V> | delete --name <NAME>`

Manage **non-secret** environment variables injected into every subprocess
Lucidos spawns (run_bash, run_python, scheduled scripts, coding agents). A
subprocess sees a change on its next spawn, with no restart. The engine loads
its own process environment from the same store only at startup. So a variable
the engine itself reads needs an engine restart.

```bash
$ lucidos env-vars list
$ lucidos env-vars set --name GITHUB_TOKEN_NOTE --value "non-secret note"
$ lucidos env-vars delete --name GITHUB_TOKEN_NOTE
```

Names must match `[A-Z_][A-Z0-9_]*` and not be engine-reserved (`CRED_*`,
`OAUTH_*`, `PATH`, internal `LUCIDOS_*`). **For secrets (API keys,
tokens, passwords) use a credential, never this**: env var values appear in
logs and events. `set` is an upsert (create-or-replace).

The chat agent has the grouped `env_vars` LLM tool (`list` / `set` / `delete`).
The retired `set_environment_variable` name still works as an alias for `set`.

### `lucidos models list | add --id <id> (--provider <p> | --routes <JSON>) [--label L] [--sort-order N] [--context-window N] [--vision true|false] | update --id <id> [...] [--reason R] | delete --id <id> [--reason R]`

Manage the chat-model registry (Settings → Models): the catalogue the Lucidos
Agent's *intelligence ladder* picks from. The model picker lists the models
with a rung that is on.

```bash
$ lucidos models list
$ lucidos models add --id z-ai/glm-5.2 --provider openrouter --label "GLM 5.2" \
    --context-window 1048576
$ lucidos models update --id z-ai/glm-5.2 --context-window 1048576
$ lucidos models update --id z-ai/glm-5.2 --on-chat-ladder false   # off the picker
$ lucidos models delete --id z-ai/glm-5.2                   # user models only
# One model, two backends: Vertex first, then OpenRouter under its own id
$ lucidos models update --id claude-opus-5-5 \
    --routes '[{"provider":"vertex"},{"provider":"openrouter","id":"anthropic/claude-opus-5-5","context_window":200000}]'
# Which backend to use when more than one route is configured
$ lucidos models update --id claude-opus-5-5 --preferred-provider openrouter
# The model reads images, so chat sends it images and image description may use it
$ lucidos models update --id z-ai/glm-5.2 --vision true
```

A provider is one of `vertex`, `anthropic`, `openai`, `openrouter`, `xai`,
`opencode-free`, `local`, `claude-gateway`, `chatgpt-plan`.

**A model has an ordered list of routes, one per backend that serves it.**
Each route names a `provider`, an optional wire `id` (default: the model id),
and an optional `context_window`. `--routes` takes the whole list as JSON, in
priority order, and replaces the stored one. A list must not be empty and must
not name a provider twice. `--provider` and `--context-window` are the
single-route shorthand: on `add` they build the one route, and on `update` they
edit the first route.

`--preferred-provider` must name one of the model's routes. It is the same
setting the picker writes when you choose a provider. A `--routes` edit that
drops the preferred provider's route clears the preference with it.

**Set `--context-window` on every model you add.** It is the model's context
window in tokens, and it sizes the engine's context budget. Without it the
engine guesses from the model id: any id containing `[1m]` gets 1M,
`gpt-` ids with major version 5 or later get 400k; other ids get 200k. OpenRouter, xAI, Gemini and local
ids get no rule, so a 1M model is trimmed at a fifth of what it holds.

Set the window your model serves for this request, not its headline maximum.
Every guess errs low on purpose: under-declaring only trims early, while
over-declaring packs a prompt the provider rejects. So most bare `claude-*` ids
sit at 200k, since Lucidos requests 1M mode only for the `[1m]` variants. Opus
5, Opus 5.5, Sonnet 5.5 and Fable 5.x are the exception: 1M is their default
window, so their bare builtin rows declare it on each Vertex and Anthropic
route.

`list` shows each model's window, or `inferred from id` when it has none.
Builtins ship with theirs declared, and accept a correction, since a vendor can
raise a window and a seeded value can be wrong. Clearing one back to inferred is
API-only: send `"context_window": null` to `PUT /api/v1/models`.

**`--vision` says whether the model reads images.** A chat turn withholds images
from a row with it off, and puts a short note in each image's place. Image description lists, defaults to and runs only such models. `add` without it means `false`, and
`update` without it keeps the stored value. Builtins ship it on the Claude,
Gemini and GPT-5/6 rows. The rest are unverified, so set it once you know.

**`list` shows each model's default effort**: the reasoning effort it runs at
when nothing stored names one, seeded from the provider's documented default.
"provider default" means none is documented, so the engine sends no effort. It
is not editable here; store a chat tier per model in `chat_reasoning_efforts`.

**`--on-chat-ladder` puts the model on the Lucidos Agent's intelligence
ladder, or takes it off.** On adds one rung, the model's default effort, else the tier nearest
`medium` its first route takes. Off turns off every rung that runs the model,
and is refused for the last model on the ladder. Either way the whole ladder is
saved as the `intelligence_ladder_chat` override. A model you add starts off.
Deleting a model takes it off the ladder first, so the last model there cannot
be deleted.

Both are ladder edits, so an agent needs the owner's `edit-intelligence-ladder`
permission (§ "Editing a ladder"). With none, the command changes nothing,
prints a request id and exits 2. Ask your question tool with that id alone,
then run the same command again once the owner allows it. Give `--reason`: the
card quotes it, and a card is refused without one.

```bash
$ lucidos models update --id z-ai/glm-5.2 --on-chat-ladder false \
    --reason "The owner asked to swap it for GLM 5.3."
```

Builtin models can leave the Lucidos Agent's ladder but not the registry. They accept route
edits (`--routes`, `--provider`, `--context-window`), `--vision` and a
`--preferred-provider`, because which backends serve a model changes, and a
seeded flag can be wrong. They
cannot be renamed, re-sorted or deleted: their identity is engine-owned. To
change the **default** chat model for new threads, set the `chat_model`
preference instead. A running thread reuses its own last-used model (see
`preferences.md`). The chat agent's `manage_models` tool runs these same
routes, so it meets the same permission.

### `lucidos repositories list`

List the *repositories* registered with this workspace: the external git repos a
*coding-agent thread* can work on. Read-only.

```bash
$ lucidos repositories list
```

Each row carries `id`, `name`, `path`, `description`, `root_commit_sha` and
`created_at`. The `id` comes from the repo's root-commit SHA, so it survives a
move, a rename and a re-clone. Compare `path` when a script has found a local
clone and needs to know whether Lucidos already knows about it.

**Registering and unregistering are not CLI ops.** They are the chat agent's
`manage_repositories` tool (`add` / `remove`). Adding a repo changes what coding
agents may touch, so the CLI stops at reading.

An app UI has no equivalent. `/api/v1/repositories` is not app-reachable, so the
engine refuses `lucidos.request` from an app frame (ADR 0231). A script running
as a subprocess is a different caller, and uses this subcommand rather than a
hand-rolled GET.

### `lucidos permissions list [--all] | revoke --permission-id <uuid>`

List the *permissions* the owner's answers to *permission requests* granted, or
revoke one. `list` shows live ones, newest first. `--all` adds the ended ones.

```bash
$ lucidos permissions list
$ lucidos permissions revoke --permission-id <uuid>
```

Each row carries `permission_id`, `action`, `breadth`, `holder_kind` and
`holder_id`, `granted_in_thread_id` and `granted_at`. The `lifetime` is `once`,
`this_thread`, `until_plan_lands` or `standing`. An ended row adds `ended_at`
and `end_reason` (`spent`, `revoked` or `plan_landed`).

Any identified caller may revoke an allow. Removing a *standing deny* restores
access, so it requires the workspace owner's attestation. Use **Settings →
Permissions → Standing denies** to edit those rules from the app.
Revoking an ended permission answers `{ "revoked": false, "reason": "already_ended" }`.

There is no chat tool for this. The Lucidos Agent runs the CLI from its shell
tool. An app frame cannot reach `/api/v1/permissions`.

