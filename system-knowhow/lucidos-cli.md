---
name: Lucidos CLI (`lucidos`)
description: Shell command available on PATH for any subprocess Lucidos spawns (Python, bash, Claude Code, Codex) — writes files under `data/`, emits and queries domain events, lists thread summaries (`lucidos threads list/count`), spawns threads (`lucidos spawn-thread`, including Codex via `--codex` / `--coding-agent codex` and app coding-agent threads via `--folder data/apps/<id>`), lists and applies pending changes (`lucidos changes list` / `lucidos changes apply <id>`), reads system-knowhow on demand (`lucidos knowhow list` / `lucidos knowhow read <id>` — how an app coding-agent thread fetches app-building guides), and calls external APIs through the engine proxy so credentials never appear in script source, args, env vars, or logs. Prefer this over hand-rolling HTTP calls back to the engine and over `curl -H "Authorization: Bearer $CRED_..."`.
---

# `lucidos` CLI

A shell command (`lucidos`) available on the `PATH` of every subprocess Lucidos spawns — Python scripts, bash scripts, coding-agent sessions (Claude Code or Codex). Use it whenever a script needs to:

- write files into the workspace's `data/` directory
- emit or query domain events on the workspace's event store
- list or count *thread summaries* in the workspace — useful for "is anything still running?" gates in triggers
- spawn a new *thread* — a chat thread, or a *coding-agent thread* on a repo or an app folder (`--cc` for Claude Code, `--codex` / `--coding-agent codex` for Codex, `--folder data/apps/<id>` for app worktrees) — `lucidos spawn-thread`
- list pending / applied *changes* (`lucidos changes list`) and apply a pending one (the coding-agent-proposed branch waiting on the Apply button) — `lucidos changes apply <id>`
- read engine-shipped system-knowhow (and user knowhow) — `lucidos knowhow list` / `lucidos knowhow read <id>` — the way an *app coding-agent thread* (whose worktree can't see `system-knowhow/`) pulls app-building guides on demand
- call an external API that's configured in `data/config/apis.json` (auth header injected by the engine — credential never appears in the script)
- send a push notification to the user without going through an LLM thread

The CLI is a thin Rust wrapper around the engine's HTTP API and filesystem conventions — for app UI usage see the JS [`lucidos.data.*`](./js-sdk.md) reference. Scripts should always prefer the CLI over hand-rolling HTTP calls back to the engine.

Some subcommands are hidden and engine-internal. They are documented here only
so the workspace-facing CLI surface stays complete; scripts and users should not
invoke them directly.

## When to use this

- **Python scripts** (`apps/<name>/scripts/*.py`, `triggers/<name>/scripts/*.py`, `knowhow/*/scripts/*.py`) — invoke via `subprocess.run(['lucidos', 'data', 'write', ...])`. The CLI is on PATH and `LUCIDOS_WORKSPACE` is set automatically.
- **Bash scripts** (same locations, `*.sh`) — call `lucidos` directly. PATH is set up before the script runs.
- **Coding-agent subprocesses** — running in a worktree under `<workspace>/.lucidos/worktrees/<id>/`, where editor writes land in the worktree (not the workspace), so dev-server links 404. Use `lucidos data write` instead.

For app UIs (in the browser) keep using the JS SDK (`lucidos.data.*`, `lucidos.events.*`). The CLI is for shell / subprocess contexts only.

## Subcommands

### Hidden: `lucidos coding-agent-diff-hook`

Internal callback installed as a git `post-commit` hook in Lucidos-managed
coding-agent worktrees. When a coding-agent process has `LUCIDOS_THREAD_ID` set,
the callback posts the current repo root and branch to the parent engine so
`coding_agent_has_diff` can refresh immediately after a commit. It is silent,
best-effort, and does not create a `ChangeProposed` event or an Apply-able
change; formal proposal still happens when the coding-agent turn idles.

Do not call this from scripts. It is scoped to the engine-installed hook and
requires the Lucidos subprocess-origin headers that the CLI attaches in spawned
subprocesses.

### `lucidos data path <relative> [--mkdir]`

Print the absolute filesystem path that `<relative>` resolves to inside the parent workspace's `data/` directory.

Path normalization matches `normalizeDataPath()` in the artifacts UI: paths starting with `artifacts/`, `knowhow/`, `apps/`, or `triggers/` are kept; anything else is prefixed with `artifacts/`.

```bash
$ lucidos data path artifacts/data-analysis/foo/report.html
/Users/.../workspaces/myws/data/artifacts/data-analysis/foo/report.html

$ lucidos data path report.html
/Users/.../workspaces/myws/data/artifacts/report.html

$ lucidos data path knowhow/myapp/notes.md --mkdir
/Users/.../workspaces/myws/data/knowhow/myapp/notes.md
# (parent dir created)
```

`--mkdir` creates the parent directory chain. Useful when piping the result to another tool.

### `lucidos data write <relative> [--from <local-path> | -]`

Write content to the resolved absolute path. Creates parent dirs automatically.

```bash
# from a local file you generated elsewhere
$ lucidos data write artifacts/data-analysis/2026-04-20/report.html --from /tmp/report.html

# from stdin (default; also `--from -`)
$ echo '{"hello": "world"}' | lucidos data write artifacts/foo.json
```

Two outputs:

- **stderr** — the resolved absolute filesystem path, so it can be captured separately (`… 2>/tmp/path`).
- **stdout** — a ready-to-paste clickable Lucidos chat link, mirroring `lucidos spawn-thread`:

```bash
$ echo '# notes' | lucidos data write artifacts/ticket-workflow/node-types-and-attributes.md
[node-types-and-attributes.md](artifacts/ticket-workflow/node-types-and-attributes.md)   # stdout
/Users/.../workspaces/myws/data/artifacts/ticket-workflow/node-types-and-attributes.md   # stderr
```

**Linking an artifact in chat — use the bare store path, never a scheme.** The clickable form is the `data/`-rooted path with no URL scheme (e.g. `artifacts/ticket-workflow/node-types-and-attributes.md`, or with the leading `data/`); the frontend's path linkifier rewrites it into a file-preview link. There is **no `artifact:` or `file:` scheme** — inventing one (by analogy to `thread:`/`app:`) produces a dead link the browser can't resolve. Paste the stdout link verbatim, or keep its target and swap the label for something friendlier: `[OST node types & attributes](artifacts/ticket-workflow/node-types-and-attributes.md)`.

### `lucidos events emit <EventType> --payload <json> [--summary <str>]`

POST a domain event to the parent workspace's event store.

- `event_type` is PascalCase past tense (`AnalysisCompleted`, `DataImported`).
- `payload` must be a JSON **object** containing a `summary` string. If you pass `--summary`, it overrides / injects `summary` into the payload before sending.

```bash
$ lucidos events emit AnalysisCompleted \
    --summary "Data analysis for 2026-04-20 complete" \
    --payload '{"artifact": "artifacts/data-analysis/2026-04-20/report.html", "rows": 1240}'
```

The CLI prints the server's JSON response on stdout (`{"success": true, "event_id": "..."}`).

### `lucidos events query [--type T] [--since iso] [--until iso] [--before-event-id UUID | --after-event-id UUID] [--limit N]`

GET events from the parent workspace's event store. Outputs the raw JSON array on stdout, newest-first.

```bash
$ lucidos events query --type AnalysisCompleted --limit 1 | jq '.[0]'
{
  "id": "...",
  "event_type": "AnalysisCompleted",
  "payload": { "summary": "UA analysis for ...", "artifact": "artifacts/..." },
  "created": "2026-04-20T12:00:00Z"
}
```

`--since` / `--until` are ISO 8601 (e.g. `2026-04-01T00:00:00Z`). `--limit` is clamped to `1..=1000` server-side, default 100.

#### Stable paging with `--before-event-id` / `--after-event-id`

Walking `--until` backwards through time is fragile at the page boundary: events sharing one timestamp get duplicated or dropped depending on inclusivity. The cursor flags solve that by ordering results lexicographically on `(created, id)` — guaranteed stable even when many events share a millisecond.

- `--before-event-id <UUID>` — return only events strictly older than that event. For backwards-walk paging.
- `--after-event-id <UUID>` — return only events strictly newer than that event. For tail-following.

The two are mutually exclusive (`400 Bad Request` if both are passed); a non-existent cursor is `404 Not Found` rather than a silently empty result.

```bash
# Page 1 — newest 100.
PAGE=$(lucidos events query --type BrowserLearningObserved --limit 100)
OLDEST_ID=$(echo "$PAGE" | jq -r '.[-1].id')

# Page 2+ — strictly older than the last event of the previous page.
lucidos events query --type BrowserLearningObserved --limit 100 \
  --before-event-id "$OLDEST_ID"
```

For tail-following, save the newest id and ask for anything strictly newer:

```bash
NEWEST_ID=$(lucidos events query --type SomeEvent --limit 1 | jq -r '.[0].id')
# ... time passes ...
lucidos events query --type SomeEvent --after-event-id "$NEWEST_ID"
```

### `lucidos events count [--type T] [--since iso] [--until iso]`

Count events by type/time without materialising payloads. Mirrors the `count_events` LLM tool. Two shapes:

- **With `--type`:** `{"count": N, "byte_total": B}` for that single type.
- **Without `--type`:** `{"by_type": [{"event_type": "...", "count": N, "byte_total": B}, ...], "total_count": N, "total_byte_total": B}` — per-type breakdown sorted by `count` desc.

```bash
# What's noisy in the last 7 days?
$ lucidos events count --since 2026-05-18T00:00:00Z | jq '.by_type[:5]'
[
  {"event_type":"ContextCaptured","count":5783,"byte_total":119537664},
  {"event_type":"ToolResult","count":4434,"byte_total":21856992},
  ...
]

# How big is one type?
$ lucidos events count --type ToolResult --since 2026-05-18T00:00:00Z
{"count":4434,"byte_total":21856992}
```

`byte_total` is `SUM(octet_length(payload::text))` — the raw payload byte sum, a reliable proxy for the token cost of a corresponding `lucidos events query` call. Use this before `query` on busy workspaces to budget which types to drill into (the recurring `workspace-learning` recipe failure that motivated this CLI was a `query --type ToolResult --limit 300` call returning 2.3 MB and blowing the next-turn prompt cap).

### `lucidos threads list [--active] [--source <list>] [--limit N]`

List thread summaries from the parent workspace. Outputs the raw JSON array on stdout, newest-first by `last_activity`. Each row is a full `ThreadSummary` — the same shape returned by the `list_threads` LLM tool and by `lucidos.threads.list()` in the JS SDK, and the same shape the projection stores in `thread_summaries`.

```bash
$ lucidos threads list --active --limit 5 | jq '.[].title'
"Plan dinner"
"Refactor settings dialog"
```

- `--active` restricts to threads where the agentic loop is mid-flow — status `running` or `waiting_for_user_answer`. Status `waiting` is **not** active: it means the coding-agent thread has stopped and proposed changes the user must act on (the loop has paused). Status `failed` is also excluded — the response is over.
- `--source` is a comma-separated list of `chat`, `trigger`, `coding-agent`. Legacy `claude_code` is also accepted. Omit for all sources.
- `--limit` clamps to `1..=1000` server-side, default 100.

Use this from a script that needs to react to thread state — e.g. "is anything still running before I fire this trigger?" — without reconstructing it from raw `query_events`. The projection already tracks per-thread status; the list endpoint is just a read off it.

### `lucidos threads count [--active] [--source <list>]`

Count thread summaries matching the same filters as `list`. Outputs `{"count": N}` on stdout.

```bash
# How many active threads in the workspace?
$ lucidos threads count --active
{"count":3}

# Is anything still running? (shell-friendly form)
$ if [ "$(lucidos threads count --active | jq .count)" -eq 0 ]; then
>   echo "Workspace is idle."
> fi
```

Cheaper than materialising the full list just to read `.length` on big workspaces.

### `lucidos spawn-thread --to <WS> --message <M> [--cc | --codex | --coding-agent <backend>] [--folder <path> | --repo <name>] [--relation child|top] [--title <T>] [--model <M>] [--cc-model <M>]`

Start a new *thread* in another (or this same) workspace — a *chat thread* by default, or a *coding-agent thread* with a coding-agent flag. `--to` names the target workspace (resolved under `$LUCIDOS_WORKSPACES_ROOT`, or an absolute path). Caller provenance (`caller_*` fields) defaults from `$LUCIDOS_WORKSPACE` / `$LUCIDOS_THREAD_ID` / `$LUCIDOS_EVENT_ID`, which the engine sets on every spawned subprocess. Prints a clickable `[title](thread:<ws>/<uuid>)` markdown link on stdout.

`--relation top` (the default) starts an independent thread that does not report back; `--relation child` is a same-workspace parent-with-callback spawn (the calling thread auto-resumes when the child finishes).

**Coding-agent backend:**

- `--cc` — legacy shortcut for a Claude Code coding-agent thread.
- `--codex` — shortcut for a Codex coding-agent thread; implies coding-agent mode and sends `coding_agent: "codex"` to the engine.
- `--coding-agent <backend>` — explicit backend selector. Valid values are `claude-code` (alias `claude_code`) and `codex`; this also implies coding-agent mode.

**Worktree targeting for coding-agent threads:**

- `--repo <name|uuid>` — create the worktree from a registered *repository*. Defaults from `$LUCIDOS_REPO` (the engine sets it to the calling thread's repo) so a coding-agent sidequest stays in its caller's repo. Pass `--repo ""` to force the target workspace's default repo.
- `--folder <path>` — target an app folder instead, spawning an **app coding-agent thread**. A `data/apps/<id>` value (workspace-relative, resolved on the *target* workspace) creates a sparse-checkout worktree narrowed to that app folder whose *Apply* ff-merges into the workspace's `main` — no `/harden`, no engine restart. This is the same machinery the `run_coding_agent` tool's `folder` argument produces. Only whole app folders are valid; the engine rejects other `data/` subtrees, app subpaths, and non-existent folders.

`--folder` and `--repo` are mutually exclusive, and `--folder` requires a coding-agent flag (`--cc`, `--codex`, or `--coding-agent`; the CLI errors before any HTTP round-trip otherwise). When `--folder` is set the `$LUCIDOS_REPO` default is suppressed — the engine rejects a request that carries both a repo and a folder.

```bash
# Spawn an app coding-agent thread to work on an app in this workspace.
$ lucidos spawn-thread --to myws --cc --relation top \
    --folder data/apps/habit-tracker \
    --title "Research session" \
    --message "Run one research session per data/apps/habit-tracker/knowhow."
[Research session](thread:myws/2f1c…)

# Spawn a Codex coding-agent thread in the dev workspace.
$ lucidos spawn-thread --to dev --codex --relation top \
    --title "Codex review" \
    --message "Review the current app folder and fix the failing test."
[Codex review](thread:dev/7a42…)
```

### `lucidos notify --title <T> --message <M> [--app-id <APP>] [--tap <T>] [--thread-id <UUID>] [--event-id <UUID>]`

Send a push notification via the parent workspace. Persists to the inbox AND fans out as a web push to subscribed devices — identical to a `send_notification` LLM tool call, but callable directly from any subprocess (Python script, bash script, scheduled `script:`-typed trigger) without going through an LLM thread.

```bash
$ lucidos notify --title "Nightly backup done" --message "Backup completed: 1,240 rows archived"
{"success":true,"notification_id":"5b1e..."}
```

Both `--title` and `--message` are required and must be non-empty (the engine returns 400 on empty values).

`--app-id <id>` is optional and stamps the notification's deep-link target. **Only set it when tapping the notification should open that app to act on it** — most reminders / nudges / summaries shouldn't deep-link, even when their trigger lives inside an app dir for organizational reasons. Same rule the `send_notification` LLM tool follows.

#### Deep-linking back to the originating event

For event-driven triggers ("coding agent is asking", "credential needed", …) the right behaviour is for the push tap to scroll straight to the specific event card the user needs to act on. Three flags wire this up:

- **`--tap <modal|navigate>`** — which kind of tap. `modal` (default) opens the inbox detail; use it for purely informational pushes too ("Backup complete", "Sync finished") — every notification is openable, there is no passive kind. `navigate` deep-links to the target inferred from the other flags: `--thread-id` → navigate to that thread (scrolling and pulsing `--event-id` when set); `--app-id` → navigate to that app. When both `--thread-id` and `--app-id` are present, thread wins (the more common CTA shape — "answer this question"). (The passive `none` kind was retired — `docs/plans/2026-07-02-remove-notification-tap-none.md`.)
- **`--thread-id <UUID>`** — the originating thread. With `--tap navigate`, the tap deep-links straight to this thread instead of the inbox modal. Even without `--tap`, this stamps the notification so the modal's "Open thread" button resolves.
- **`--event-id <UUID>`** — a specific event id inside `--thread-id` to scroll to and briefly pulse when the tap lands. Ignored when `--thread-id` is absent.

```bash
# Deep-link the push to the exact UserQuestionAsked card on tap.
lucidos notify \
  --title "Coding agent is asking" \
  --message "Ship it?" \
  --tap navigate \
  --thread-id "$TRIGGER_EVENT_THREAD_ID" \
  --event-id "$TRIGGER_EVENT_ID"
```

The `TRIGGER_EVENT_THREAD_ID` and `TRIGGER_EVENT_ID` env vars in the snippet are set by the engine on every script trigger fired by a thread-scoped event (see `building-a-trigger.md` § "Script trigger env vars"). For schedule-fired triggers neither is set, so `--tap modal` (the default) is the only meaningful choice.

The CLI rejects `--tap navigate` without `--thread-id` and `--app-id` (the navigate kind needs a destination) with a clear error before the HTTP round-trip — the server returns the same 400 if the CLI's check is bypassed. For panel-shaped targets (`changes`, `triggers`, `files`, …) the CLI doesn't currently expose a flag — use the `send_notification` LLM tool or POST directly to `/api/v1/notifications` with the full structured `tap` object.

#### Response and exit codes

The CLI prints the engine's JSON response on stdout (`{"success": true, "notification_id": "<uuid>"}`). Non-zero exit on transport / HTTP error, with the engine's error body (or `lucidos: <transport error>`) on stderr.

#### When to use which

| Context | Use |
|---|---|
| Scheduled `script:`-typed trigger that needs to nudge the user | `lucidos notify` |
| One-off bash / Python script run as part of an app or trigger | `lucidos notify` |
| LLM agent in a chat / trigger thread | `send_notification` tool (LLM picks `app_id` based on context) |
| Background engine code (Rust) | `LucidosEngine::create_notification` (the shared helper both surfaces call) |

### `lucidos notifications list | read --id <uuid> | read-all`

Read and clear the notification *inbox* (the complement of `notify`, which only
*sends*). Generated from the capability parity manifest, so it routes through the
same gateway-safe HTTP client as every other subcommand — use it instead of
hand-rolling `curl`, which has to reverse-engineer the engine port and the
gateway `/<slug>/` path prefix.

```bash
# What's unread? (default filter is unread; pass --filter all for everything)
$ lucidos notifications list
[ { "id": "c3dac86b-…", "title": "Backup failed", "message": "…", "read": false, "created_at": "…" } ]

# Clear one by id (from the list above)
$ lucidos notifications read --id c3dac86b-bfd1-4f1e-a9d2-b47567957d25

# Clear the whole unread inbox
$ lucidos notifications read-all
```

`list` accepts `--filter unread|all` and `--limit N` (1–50, default 20). `read`
requires `--id <uuid>`. Both `read`/`read-all` emit `NotificationRead` /
`NotificationsAllRead` so other devices' unread state syncs over SSE. Exit
non-zero on transport / HTTP error.

> **In-thread agent:** the chat Lucidos Agent has the equivalent grouped
> `notifications` tool (`action: list | mark_read | mark_all_read`), which runs
> **in-process** (no HTTP round-trip). Use the tool from a chat / trigger thread;
> use this CLI from a `script:`-typed trigger or a coding-agent / bash / Python
> subprocess. Both surfaces are generated from / checked against the same
> capability parity manifest, so they can't drift.

### `lucidos preferences get | set --key <K> --value <V>`

Read and change user *preferences* (Settings). Generated from the capability
parity manifest (gateway-safe HTTP client). `get` lists every settable key with
its current value, allowed values, default, and scope; `set` changes one.

```bash
$ lucidos preferences get
$ lucidos preferences set --key timezone --value Europe/Oslo
$ lucidos preferences set --key chat_model --value claude-opus-4-8@default
```

`get` accepts `--device-id <id>` (read device-scoped overrides; omit for the
global view). `set` requires `--key` + `--value`; pass `--device-id` only for a
per-device key. The chat agent's in-process equivalent is the grouped
`preferences` tool (`action: get | set`).

### `lucidos triggers list | create | update | delete`

Manage *triggers* — scheduled (cron) and/or event-driven automations. Generated
from the manifest. The rich fields (`run`, `on`, `cron_expressions`,
`side_effect_grant`) are passed as JSON strings.

```bash
$ lucidos triggers list
# Create a daily 8am intent trigger (cron in the user's local timezone)
$ lucidos triggers create --name "Morning digest" \
    --run '{"type":"intent","intent":"summarise overnight emails"}' \
    --cron-expressions '["0 0 8 * * *"]'
# Event-driven trigger with a payload filter
$ lucidos triggers create --name "Bad sleep alert" \
    --run '{"type":"intent","intent":"nudge me to rest"}' \
    --on '[{"event_type":"OuraSleepImported","condition":{"sleep_score":{"$lt":70}}}]'
# Update keeps run history (prefer over delete+create); pause/resume via --paused
$ lucidos triggers update --id <uuid> --paused true
$ lucidos triggers delete --id <uuid>
```

`create`/`update` accept `--name`, `--run`, `--cron-expressions`, `--on`,
`--app-id`, `--go-to-review`, `--group-id`, `--side-effect-grant`, `--slug`;
`update`/`delete` take `--id <uuid>`. The chat agent's in-process equivalent is
the grouped `triggers` tool (`action: create | list | update | delete | pause |
resume`) — pause/resume are tool-only (the CLI pauses via `update --paused`).

### `lucidos trigger-groups list | create | rename | reorder | delete`

Manage *trigger groups* — the user-visible folders that organize triggers in the
panel (pure organizational label; no firing).

```bash
$ lucidos trigger-groups list
$ lucidos trigger-groups create --name "Health" --order 10
$ lucidos trigger-groups rename --id <uuid> --name "Wellbeing"
$ lucidos trigger-groups reorder --ordering '[{"id":"<uuid>","order":0}]'
$ lucidos trigger-groups delete --id <uuid>
```

Assign a trigger to a group with `lucidos triggers update --id <uuid>
--group-id <group-uuid>`. The chat agent's in-process equivalent is the grouped
`trigger_groups` tool.

### `lucidos apps list | get --id <id> | update | delete`

Manage *apps* — list, inspect, rename, or delete. (Creating an app and editing
its source are not CLI ops: creation is the chat agent's `create_app` tool, and
source editing happens in the app's coding-agent worktree.)

```bash
$ lucidos apps list
$ lucidos apps get --id habit-tracker
$ lucidos apps update --id habit-tracker --name "Habit Tracker" --description "Daily habits"
$ lucidos apps delete --id habit-tracker
```

`get`/`update`/`delete` take `--id`; `update` takes `--name` (required) +
`--description`. Plugin-installed apps refuse `delete` (remove the plugin
instead). `list`/`get` are also in the JS SDK (`lucidos.apps`); `update`/`delete`
are CLI-only.

### `lucidos thread-queue list | run-now --entry-id <uuid> | drop --entry-id <uuid>`

Inspect the *Thread Queue* (background admission control) — `list` prints the
live queue + active *capacity policy* as JSON; `run-now` force-admits a queued
entry ignoring caps; `drop` removes a queued entry without running it.

```bash
$ lucidos thread-queue list
$ lucidos thread-queue run-now --entry-id 0b1e…  # force-admit
$ lucidos thread-queue drop --entry-id 0b1e…     # cancel a queued entry
```

Get an entry id from `list` (`entries[].id`). Mirrors the chat agent's grouped
`thread_queue` tool. Changing the capacity policy is the LLM tool's
`update_policy` action — deliberately **not** a CLI command, because the raw
`PUT /thread-queue/policy` replaces omitted caps with defaults (the LLM tool
merges with the live policy instead).

### `lucidos memory stats | entries [--limit N] [--offset N] [--source-type T] [--sort S] [--importance L] | source [--source-id UUID] [--source-type T] [--path P] [--commit C]`

Read long-term memory — `stats` (index counts), `entries` (paginated entries
with importance + source), `source` (the originating event/artifact for one
memory plus the entries derived from it). All read-only.

```bash
$ lucidos memory stats
$ lucidos memory entries --limit 20 --importance high,critical
$ lucidos memory source --source-type event --source-id <uuid>
```

Correcting memory is the chat agent's grouped `memory` tool (`correct` /
`correct_by_id`), not a CLI op — and the agent gets memory injected into its
context, so these reads are for subprocess inspection.

### `lucidos env-vars list | set --name <NAME> --value <V> | delete --name <NAME>`

Manage **non-secret** environment variables injected into every subprocess
Lucidos spawns (run_bash, run_python, scheduled scripts, coding agents).

```bash
$ lucidos env-vars list
$ lucidos env-vars set --name GITHUB_TOKEN_NOTE --value "non-secret note"
$ lucidos env-vars delete --name GITHUB_TOKEN_NOTE
```

Names must match `[A-Z_][A-Z0-9_]*` and not be engine-reserved (`CRED_*`,
`OAUTH_*`, `PG*`, `PATH`, internal `LUCIDOS_*`). **For secrets (API keys,
tokens, passwords) use a credential, never this** — env var values appear in
logs/events. `set` is an upsert (create-or-replace).

At parity, the chat agent has the grouped `env_vars` LLM tool (`list` / `set` /
`delete`) — the retired `set_environment_variable` name still works as a
back-compat alias for `set`.

### `lucidos models list | add --id <id> --provider <p> [--label L] [--sort-order N] [--context-window N] | update --id <id> [...] | delete --id <id>`

Manage the chat-model registry (Settings → Models) — the models in the Lucidos
Agent's picker.

```bash
$ lucidos models list
$ lucidos models add --id z-ai/glm-5.2 --provider openrouter --label "GLM 5.2" \
    --context-window 1048576
$ lucidos models update --id z-ai/glm-5.2 --context-window 1048576
$ lucidos models update --id z-ai/glm-5.2 --enabled false   # disable
$ lucidos models delete --id z-ai/glm-5.2                   # user models only
```

`provider` is one of `vertex`, `anthropic`, `openai`, `openrouter`, `local`.

**`--context-window` is worth setting on every model you add.** It's the model's
context window in tokens, and it sizes the engine's context budget. Omit it and
the engine falls back to guessing from the model id (`claude-*` → 200k unless the
id carries `[1m]`, `gpt-5*` → 400k, anything else → 200k). That guess has no rule
at all for OpenRouter, Gemini, or local ids, so they are treated as 200k however
large they really are — a 1M model gets its context trimmed at a fifth of what it
could hold.

Set it to the window your model actually serves for the request being made, not
its headline maximum. Every guess errs low on purpose: under-declaring only trims
early, whereas over-declaring makes the engine pack a prompt the provider then
rejects. (This is why bare `claude-*` ids sit at 200k rather than the 1M those
models advertise — Lucidos requests 1M mode only for the `[1m]` variants.)

`list` shows each model's window, or `inferred from id` when it has none.
Builtins ship with theirs already declared. Builtins accept a window correction
too — the vendor can raise a model's window, and a seeded value can be wrong.
(Clearing one back to inferred is API-only — send `"context_window": null` to
`PUT /api/v1/models`; there's no CLI flag for it.)

Builtin models can be disabled (`update --enabled false`) and can have their
context window corrected, but they can't be renamed, re-providered, or deleted —
their identity is engine-owned. To
change the **default** chat model for new threads, set the `chat_model`
preference instead (a thread that's already running reuses its own last-used
model — see `preferences.md`). Mirrors the chat agent's `manage_models` tool.

### `lucidos changes list`

List pending and recently-applied *changes*. Wraps `GET /api/v1/changes` and echoes the engine's payload verbatim to stdout. This is the canonical way for a script to find a pending change's id before `apply` — read `.pending[].id`. Don't scan `ChangeProposed` events for the id when this one command gives it directly.

```bash
$ lucidos changes list
{"pending":[{"id":"fbcc4a3a-...","branch_name":"claude-code/...","description":"fix: …","status":"pending",...}],"applied":[...],"total_pending":1,"restart_required":false,"restart_groups":[],"client_update_available":false,"has_more_applied":false}

# Find the single pending change's id (e.g. in a build → apply pipeline):
$ CID=$(lucidos changes list | jq -r '.pending[0].id')
$ lucidos changes apply "$CID"
```

The response carries `pending` (array of pending changes, each with `id` / `branch_name` / `description` / `status` / `file_count` / `requires_restart` / `thread_id`), `applied` (recently applied), `total_pending`, and `restart_required`. Exit non-zero on transport / HTTP error.

> **In-thread agent:** the chat Lucidos Agent has the equivalent `list_changes` LLM tool, which returns the same `{pending, applied, total_pending}` shape **in-process** (no HTTP round-trip). Use `list_changes` from a chat / trigger thread; use this CLI from a `script:`-typed trigger or a bash / Python subprocess.

### `lucidos changes apply <change-id>`

Apply a pending *change* (a coding-agent-proposed branch that's waiting on the Apply button). Wraps `POST /api/v1/changes/<id>/apply` and echoes the engine's typed `ApplyChangeResult` JSON to stdout. Get the id from `lucidos changes list` (`.pending[].id`).

```bash
$ lucidos changes apply fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec
{"status":"applied","change_id":"fbcc4a3a-...","thread_id":"1c1c34ef-...","message":"Change applied.","restart_required":false,"applied_commit":"9b1a...","previous_commit":"2a3b...","commits_applied":3,"files_changed":5}
```

> **In-thread agent:** the chat Lucidos Agent has the equivalent `apply_change` LLM tool. It calls the same engine apply pipeline **in-process** and stamps the apply as the agent (linked back to the applying thread), so the route popover never mislabels it as "You". Use `apply_change` from a chat / trigger thread; use this CLI from a `script:`-typed trigger or a bash / Python subprocess (which can't call the in-process tool and would otherwise have to forward the subprocess-origin headers by hand).

The response carries:

| Field | Meaning |
|---|---|
| `status` | `applied`, `noop`, `hardening`, or `conflict` (see `docs/apply-change-api.md` for the full table) |
| `applied_commit` | 40-char SHA on `main` AFTER the merge (present on `applied` and idempotent `noop`) |
| `previous_commit` | 40-char SHA on `main` BEFORE the merge |
| `commits_applied` | Number of commits added to `main` (0 for `noop`) |
| `restart_required` | `true` when the changed files trigger an engine restart on apply |
| `conflict_thread_id` / `review_thread_id` | Thread to focus when `status` is `conflict` / `hardening` |

The CLI prints the JSON verbatim on stdout. Exit non-zero on transport / 4xx with the engine's error body on stderr — match `--fail` semantics from `lucidos proxy`.

Two 409s are refusals rather than errors, and both name the resolution: the change's thread is still working (wait for it to idle), or the change has **no file changes left** (`file_count` is 0 — its branch's commits cancelled out, so there is nothing to merge; discard it with the Discard button instead). A script driving a build → apply pipeline should treat a zero-`file_count` entry in `lucidos changes list` as "nothing to apply", not as a change to retry.

#### Why use the CLI instead of hand-rolled urllib / curl

The CLI auto-forwards two subprocess-origin headers (`x-lucidos-agent-origin-token`, `x-lucidos-source-thread-id`) that the engine reads to stamp the resulting `ChangeApplied` event as `Api { mode: Agent, source_thread_id }`. Without them, the engine falls through to `Api { mode: Human }` and the UI renders the apply card as **"You"** — wrongly attributing an agent action to the user. A `run_python` block that calls `urllib.request.urlopen("https://localhost:.../api/v1/changes/<id>/apply")` will hit this bug because urllib doesn't read the env vars on its own.

```python
# ❌ Wrong — the User-Agent on the request is "Python-urllib/X.Y" and the UI says "You"
import ssl, urllib.request as r
ctx = ssl._create_unverified_context()  # self-signed cert
r.urlopen(r.Request(f"https://localhost:{port}/api/v1/changes/{cid}/apply", method="POST"), context=ctx)

# ✅ Right — CLI forwards the headers; UI says "Lucidos Agent" with the source thread linked
import subprocess
subprocess.run(["lucidos", "changes", "apply", cid], check=True)
```

The same rule applies to bash:

```bash
# ❌ Wrong — bare curl from inside a run_bash tool
curl -k -X POST "https://localhost:$LUCIDOS_API_PORT/api/v1/changes/$CID/apply"

# ✅ Right — CLI handles the headers
lucidos changes apply "$CID"
```

If a script genuinely needs to call the HTTP endpoint directly (test harness, external tool that can't shell out to the CLI), forward both headers explicitly and build the base URL from `$LUCIDOS_API_BASE_URL`:

```bash
curl -k -X POST \
  -H "x-lucidos-agent-origin-token: $LUCIDOS_AGENT_ORIGIN_TOKEN" \
  -H "x-lucidos-source-thread-id: $LUCIDOS_THREAD_ID" \
  "${LUCIDOS_API_BASE_URL:-https://localhost:$LUCIDOS_API_PORT}/api/v1/changes/$CID/apply"
```

Use `$LUCIDOS_API_BASE_URL` (set by the engine on every spawned subprocess) rather than building the URL from `$LUCIDOS_API_PORT` yourself: under the workspace gateway (ADR 0014) the engine binds a **loopback HTTP** port and the user-facing port belongs to the gateway, which routes the workspace under `/<slug>/` — a bare `https://localhost:$LUCIDOS_API_PORT/api/v1/...` request there never reaches the engine (the gateway resolves the first path segment as a workspace slug). `$LUCIDOS_API_BASE_URL` is the exact base the engine answers on (loopback `http://` under the gateway; `https://` self-signed in the legacy single-engine model, which `-k` / `_create_unverified_context()` accepts). The fallback to `$LUCIDOS_API_PORT` covers older engines that predate the var. The token env var is process-local secret state set by the engine on every spawned subprocess; the thread id is set when the subprocess has a spawning thread. See `docs/apply-change-api.md` for the response shape and the full apply workflow.

### `lucidos planned mark (--plan <path> | --simple "<reason>")` / `lucidos planned approve` / `lucidos planned state`

Record, approve, or query the *plan marker* — the durable enforcement that the `implementation-plan` skill ran AND the human approved its plan (or that a local fix was acknowledged) before a *Lucidos-source* coding-agent branch is edited and applied. A **gate-satisfying** marker MUST exist on the branch or Claude Code's first source edit is blocked (the `cc-plan-gate` PreToolUse hook) and Apply is refused (the engine's plan floor). Wraps `POST /api/v1/internal/mark-planned` / `POST /api/v1/internal/approve-plan` / `GET /api/v1/internal/planned-state`.

```bash
# Complex work: the implementation-plan skill writes the plan, then records this for you.
# This records the AWAITING-APPROVAL `proposed` state — it does NOT unblock editing:
lucidos planned mark --plan docs/plans/2026-06-19-my-change.md

# Present the plan to the user. Once the user APPROVES in chat, flip it to gate-satisfying:
lucidos planned approve

# Genuinely local fix that doesn't warrant a plan — acknowledge instead (no approval needed):
lucidos planned mark --simple "rename a misspelled variable"

# Inspect the current branch's marker (SATISFIED, PROPOSED, or MISSING):
lucidos planned state
```

`mark` / `approve` resolve repo_root / branch / HEAD from `$PWD`'s git worktree (like `lucidos hardened mark`). Pass exactly one of `--plan` / `--simple`. **`mark --plan` records `proposed` (awaiting approval) — it does NOT satisfy the gate.** The agent must present the plan to the user and, only after the user approves in chat, run `lucidos planned approve` to flip `proposed`→`planned` (gate-satisfying). If the user requests changes, revise the plan file, re-commit, and re-present (the marker stays `proposed`). `mark --simple` records `acknowledged_simple` directly — local fixes need no approval. `planned` and `acknowledged_simple` satisfy every gate; `proposed` and the absence of a marker both block. App coding-agent threads and external repos are exempt (the gate is a no-op there). Normally you don't call `mark --plan` / `approve` by hand — the `implementation-plan` skill drives them — but `mark --simple` is the agent's escape hatch for a change too small to plan. (`lucidos cc-plan-gate` is the hidden PreToolUse hook that enforces this; it is not invoked directly.)

### `lucidos knowhow list`

List the merged user + system-knowhow catalog. Wraps `GET /api/v1/knowhow` and echoes the engine's payload verbatim: `{ "knowhow": [{ "id", "name", "description" }] }`. Engine-shipped reference docs carry the `system-knowhow/` id prefix; user-curated knowhow uses its path under `data/knowhow/` without `.md`. Read `.knowhow[].id` to find the id to pass to `read`.

```bash
$ lucidos knowhow list
{"knowhow":[{"id":"audit-checklist","name":"Audit checklist","description":"..."},{"id":"system-knowhow/building-an-app","name":"Building an App","description":"Use when the user wants to build..."},...]}
```

### `lucidos knowhow read <id>`

Read one knowhow doc's full content by id. Wraps `GET /api/v1/knowhow/read?id=<id>` and prints the same `[KNOW-HOW: …]` / `[SYSTEM-KNOWHOW: …]` block the chat agent's `load_knowhow` tool returns. Exit non-zero (with the engine's not-found sentinel on stderr) when the id resolves to nothing.

```bash
$ lucidos knowhow read system-knowhow/building-an-app
[SYSTEM-KNOWHOW: Building an App]
# Building an App
…
[END SYSTEM-KNOWHOW]
```

**Why this exists.** The chat Lucidos Agent loads `system-knowhow/*.md` via its in-process `load_knowhow` tool — but an *app coding-agent thread* runs in a sparse-checkout *worktree* narrowed to a single `data/apps/<id>/` folder, so the engine's `system-knowhow/` is neither on disk nor reachable via that tool. This subcommand is how such a session (Claude Code or Codex) pulls the same app-building guidance — `system-knowhow/building-an-app` (when an app is the right answer, scaffolding defaults, common mistakes), `system-knowhow/js-sdk` (the `lucidos.*` SDK surface), `system-knowhow/best-practices` (file layout, where app data lives) — on demand. Load the relevant knowhow before writing app code rather than guessing at the SDK surface or data paths.

> **In-thread agent:** the chat Lucidos Agent uses the `load_knowhow` LLM tool with the same id (`system-knowhow/<id>`), in-process. Use `load_knowhow` from a chat / trigger thread; use this CLI from a coding-agent thread or a bash / Python subprocess that has no in-process tool access.

### `lucidos proxy <name> [path] [-X METHOD] [-H "Hdr: val"] [-d body | --data-stdin] [-i] [--fail]`

Call a backend configured in `data/config/apis.json` through the engine. The engine resolves the credential from the workspace's credential store, injects the configured auth header, and strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request. **The credential value never reaches the script** — neither in `argv`, env vars, the request line, nor any log.

**This is the preferred way for scripts to call external APIs.** The previous pattern — `curl -H "Authorization: Bearer $CRED_FOO" ...` with `$CRED_FOO` injected into the script's environment — leaks the secret into process args and shell history. Configure the API in `data/config/apis.json` once, then use `lucidos proxy` everywhere.

#### Configure the backend (one-time)

`data/config/apis.json`:

```json
{
  "sonos":   { "base_url": "http://localhost:5005" },
  "comfort": {
    "base_url": "https://accsmart.panasonic.com",
    "auth": { "type": "bearer", "credential": "comfort-cloud" }
  },
  "weather": {
    "base_url": "https://api.weather.example",
    "auth": { "type": "api_key", "credential": "weather-api", "header": "X-API-Key" }
  }
}
```

`auth.type` selects how the engine attaches the credential to the outgoing request. Six modes are supported:

- **`bearer`** — `Authorization: Bearer <auth_value>`. `{"type": "bearer", "credential": "<service_name>"}`
- **`api_key`** — `<header>: <auth_value>` (default header `Authorization`). `{"type": "api_key", "credential": "<service_name>", "header": "X-API-Key"}`
- **`basic`** — `Authorization: Basic <base64(auth_value)>`. The credential's `auth_value` should already be `user:password`. `{"type": "basic", "credential": "<service_name>"}`
- **`query_param`** — appends `?<param_name>=<auth_value>` to the request URL. Used for APIs (e.g. Helius) that take the key as a query parameter. `{"type": "query_param", "credential": "<service_name>", "param_name": "api-key"}`
- **`hmac_signed`** — signs each request with HMAC over the query string. Used for APIs (e.g. Binance) that require per-request signing. Optional `timestamp_param` injects the current millis-since-epoch as a query parameter before signing. Optional `key_header` (default `X-API-KEY`) carries the API key. `signature_param` (default `signature`) names the resulting signature parameter. `algorithm` is `sha256` or `sha512`. `signed_payload` is `query_string`. `{"type": "hmac_signed", "key_credential": "binance-key", "secret_credential": "binance-secret", "key_header": "X-MBX-APIKEY", "algorithm": "sha256", "signed_payload": "query_string", "signature_param": "signature", "timestamp_param": "timestamp"}`
- **`script_handshake`** — for APIs that need a multi-step login (POST creds, get a session token / multi-header response, refresh on a schedule). Engine spawns a per-API Python script you write under `data/scripts/auth/<api>.py`, caches the resulting headers in memory, refreshes on `expires_in` or on upstream 401. `{"type": "script_handshake", "credential": "<service_name>", "script": "scripts/auth/<api>.py"}`. The credential can be of any type; the script reads it as `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD` for `password`, or `CRED_<NAME>` for the others — same convention as `run_python` / `run_bash`. Optional `"oauth_providers": ["google", ...]` injects each listed provider's connected access token (auto-refreshed) as `OAUTH_<UPPER>_ACCESS_TOKEN` in the script's env. Missing-provider failure is a clear 502 naming the provider, so the user knows which `connect_oauth_account` to run. See `system-knowhow/building-an-auth-handshake.md` for the full guide and worked Comfort Cloud + Firebase examples.

`auth.credential` (singular variants) and `auth.credentials` / `auth.key_credential` / `auth.secret_credential` (multi-credential variants) reference `service_name`s already in the engine credential store (the same store `request_credential` writes to). Entries without an `auth` block forward unauthenticated — useful for local services like Sonos.

#### Usage (curl-style ergonomics)

```bash
# GET; body to stdout, exit 0 even on 4xx/5xx (curl convention)
lucidos proxy sonos /Spisestua/play

# POST with inline body
lucidos proxy comfort /api/v1/devices -X POST \
  -H "Content-Type: application/json" \
  -d '{"deviceGuid":"abc"}'

# POST with body from stdin
cat payload.json | lucidos proxy comfort /api/v1/devices -X POST --data-stdin

# Status line + headers + body (curl -i)
lucidos proxy sonos /zones -i

# Exit non-zero on HTTP errors and suppress body (curl --fail) — use in scripts
# that need to react to upstream failures
lucidos proxy sonos /zones --fail
```

Output is the response body on **stdout**. With `--include`, the status line and headers are prepended to stdout (curl convention — single stream). With `--fail`, the body is suppressed and a one-line `lucidos proxy: HTTP <code>` summary is written to stderr instead. Transport errors (DNS failure, connection refused, …) print to stderr (`lucidos: ...`) and exit non-zero. Exit codes mirror curl: `0` on success (including 4xx/5xx by default), `22` when `--fail` and the response is 4xx/5xx, `1` on transport failure.

`script_handshake`-typed proxies look identical to the caller — `lucidos proxy comfort-cloud /devices/list` — because the engine runs the configured login script transparently and attaches the resulting headers. See `system-knowhow/building-an-auth-handshake.md` for authoring the script.

#### When to use which

| Want to … | Use |
|---|---|
| Call a backend the workspace will reuse | `lucidos proxy` (configure once in `apis.json`, then no auth in script) |
| One-off `curl` to a service the workspace will never reuse | Plain `curl` (no proxy entry needed) |
| Emit/query domain events | `lucidos events …` |
| Write a file under `data/` | `lucidos data write …` |
| Push a notification to the user from a script | `lucidos notify --title … --message …` |
| Find a pending change's id from a script | `lucidos changes list` (read `.pending[].id`; don't scan `ChangeProposed` events) |
| Apply a coding-agent-proposed change from a script | `lucidos changes apply <id>` (never hand-roll the HTTP call — actor stamps as "You") |

If you find a script doing `curl -H "Authorization: Bearer $CRED_..."` against an API the workspace already owns a credential for, that's drift — add an `apis.json` entry and switch the script to `lucidos proxy`.

## Workspace resolution

The CLI figures out **which workspace** to talk to in this order:

1. **`$LUCIDOS_WORKSPACE`** environment variable. The engine sets this on every spawned subprocess (Python, bash, coding-agent sessions), so this is the authoritative path for the engine-spawned case.
2. **Walk up from `$PWD`** looking for the first ancestor directory that contains a `.lucidos/ports` file. Fallback for terminal users running the CLI by hand without the env var set.

You should never need to think about this — the env var is configured automatically when the engine spawns the subprocess.

**Reaching the engine API.** Once the workspace is located, the CLI picks the engine's base URL in this order:

1. **`$LUCIDOS_API_BASE_URL`** — set by the engine on every spawned subprocess when it is reachable somewhere other than the ports-file port. Under the workspace gateway (ADR 0014) the engine binds a **loopback HTTP** port while the user-facing port belongs to the gateway (which routes the workspace under `/<slug>/`), so the engine hands the CLI the exact loopback URL. Without this, a bare `https://localhost:<gateway-port>/api/v1/...` request would never reach the engine (the gateway resolves the first path segment as a workspace slug).
2. **`.lucidos/ports`** (`API_PORT` + optional `PROTO`, default `https`) — the legacy single-engine model, where the engine listens directly on the user-facing port. Used when `$LUCIDOS_API_BASE_URL` is absent (legacy / Tauri / terminal).

## Common patterns

### Write an artifact and emit a completion event

The canonical end of an analysis or report-generation session. This is the pattern an analysis app's prompt should use instead of trying to write into the worktree:

```bash
ARTIFACT="artifacts/data-analysis/$(date +%Y-%m-%d)/report.html"

# 1. Write the artifact under data/.
lucidos data write "$ARTIFACT" --from /tmp/report.html

# 2. Tell the workspace it's ready.
lucidos events emit AnalysisCompleted \
  --summary "UA analysis for $(date +%Y-%m-%d) finished" \
  --payload "{\"artifact\": \"$ARTIFACT\"}"
```

Both calls speak the same `data/`-rooted path convention, so an SSE listener that does `lucidos.data.url(payload.artifact)` in the frontend will resolve the link correctly.

### Call an external API and persist the response

The canonical "pull from a service, store under `artifacts/imported/`, signal completion" loop. Auth is handled by the engine — the script never sees the credential.

```bash
DATE=$(date +%Y-%m-%d)
ARTIFACT="artifacts/imported/comfort/$DATE/state.json"

# Configured in data/config/apis.json under "comfort" with bearer auth.
# Engine injects Authorization: Bearer <stored credential> automatically.
lucidos proxy comfort /api/v1/devices --fail \
  | lucidos data write "$ARTIFACT"

lucidos events emit ComfortStateImported \
  --summary "Imported Comfort Cloud device state for $DATE" \
  --payload "{\"date\": \"$DATE\", \"artifact\": \"$ARTIFACT\"}"
```

Compare to the pre-proxy form, which leaks the credential into argv and shell history:

```bash
# Don't do this — $CRED_COMFORT shows up in `ps`, in shell history, and
# in any log that captures the script's invocation.
curl -sf -H "Authorization: Bearer $CRED_COMFORT" \
  https://accsmart.panasonic.com/api/v1/devices > /tmp/x.json
```

### Idempotent skip-if-already-done

```bash
LATEST=$(lucidos events query --type DataImported --limit 1 | jq -r '.[0].payload.date // empty')
if [ "$LATEST" = "$(date +%Y-%m-%d)" ]; then
  echo "Already imported today; skipping."
  exit 0
fi

# ... do work ...

lucidos events emit DataImported --summary "Imported $(date +%Y-%m-%d)" \
  --payload "{\"date\": \"$(date +%Y-%m-%d)\", \"rows\": $ROWS}"
```

## How the CLI ends up on PATH

For every script the engine spawns, it:

1. Symlinks the bundled `lucidos` binary into `<workspace>/.lucidos/bin/lucidos` (idempotent — safe to call on every spawn).
2. Prepends `<workspace>/.lucidos/bin` to `PATH`.
3. Sets `LUCIDOS_WORKSPACE=<workspace>` so the CLI's fallback always resolves.

For Claude Code sessions it additionally drops a skill file at `<worktree>/.claude/skills/lucidos-cli/SKILL.md` so Claude Code discovers the CLI via its normal skill mechanism. Codex sessions receive the CLI guidance in their system prompt. A one-line reminder ("Use the `lucidos` CLI for any data-dir writes or event emits.") in your trigger/app prompt is still good belt-and-braces.

## Implementation

- Source: `crates/lucidos-cli/`
- Shared engine wiring: `crates/lucidos-engine/src/runtime/lucidos_cli.rs` — `lucidos_cli_dir` discovers the binary, `ensure_workspace_bin_symlink` installs the workspace-relative symlink, `workspace_script_env_vars` builds the env var bundle.
- Used by `claude_code.rs` (Claude Code sessions) and `engine/mod.rs::build_script_env_vars` (Python/bash tool calls + scheduled scripts).
- The CLI itself is a thin wrapper — see [`js-sdk.md`](./js-sdk.md) for the equivalent in-browser API used by app UIs.
