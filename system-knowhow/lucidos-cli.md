---
name: Lucidos CLI (`lucidos`)
description: The `lucidos` shell command, on PATH in every subprocess Lucidos spawns (Python, bash, Claude Code, Codex): write files under data/, emit and query domain events, await an event instead of polling, spawn threads, apply pending changes, and call an external API through the engine proxy.
---

# `lucidos` CLI

A shell command (`lucidos`) available on the `PATH` of every subprocess Lucidos spawns — Python scripts, bash scripts, coding-agent sessions (Claude Code or Codex). Use it whenever a script needs to:

- write files into the workspace's `data/` directory
- emit a domain event, or query the workspace's event store (which holds engine thread/system events alongside domain events, and `events query` returns both)
- list or count *thread summaries* in the workspace — useful for "is anything still running?" gates in triggers
- spawn a new *thread* — a chat thread, or a *coding-agent thread* on a repo or an app folder (`--cc` for Claude Code, `--codex` / `--coding-agent codex` for Codex, `--folder data/apps/<id>` for app worktrees) — `lucidos spawn-thread`
- subscribe the calling thread to an event instead of polling for it, and finish, letting the engine re-open the thread when the event lands: `lucidos await-event`
- read what this thread is currently subscribed to, and stop watching: `lucidos event-waits list` / `lucidos event-waits cancel`
- list pending / applied *changes* (`lucidos changes list`) and apply a pending one (the coding-agent-proposed branch waiting on the Apply button) — `lucidos changes apply <id>`
- read engine-shipped system-knowhow (and user knowhow) — `lucidos knowhow list` / `lucidos knowhow read <id>` — the way an *app coding-agent thread* (whose worktree can't see `system-knowhow/`) pulls app-building guides on demand
- call an external API that's configured in `data/config/apis.json` (auth header injected by the engine — credential never appears in the script)
- send a push notification to the user without going through an LLM thread

The CLI is a thin Rust wrapper around the engine's HTTP API and filesystem conventions — for app UI usage see the JS [`lucidos.data.*`](./js-sdk.md) reference. Scripts should always prefer the CLI over hand-rolling HTTP calls back to the engine.

## Never post to the engine API as the user, and never route around a tool

Read this before anything below it. Three rules, and the third is the one that
matters most.

1. **Never fabricate a human turn.** A message you author is an *agent* message.
   You may not POST `mode: "human"` to `/api/v1/chat/stream` (or anywhere else),
   because a turn recorded as the user is indistinguishable, in the timeline and
   in the event log, from something they actually typed. It also lands in the
   projection as a user action: it sets the thread's initiator to the user and
   bumps the drawer's recency sort. The engine refuses a human claim it has no
   evidence for, but the rule binds you whether or not the engine catches you.

2. **Never hand-roll HTTP to the engine to get past a restriction.** If a tool
   or a CLI subcommand will not let you do something, that is the answer, not an
   obstacle. Reaching for `curl`, `urllib` or `fetch` against the engine's own
   `/api/v1` surface to do the same thing another way is the failure mode this
   section exists for. Use the CLI (it forwards the attribution headers the
   engine reads, and it resolves the right engine) and stop where it stops.

3. **When a tool refuses you, TELL THE USER IT IS NOT POSSIBLE.** This is the
   part that was missing on 2026-08-06, when an agent asked to message every
   running coding-agent thread found that `follow_up_child_thread` and
   `lucidos threads follow-up` reach only its own children, and instead of
   saying so it curled the engine directly. Six agent messages were recorded as
   the user's. They also went to the wrong engine (another workspace's, on a
   guessed port), which created six phantom threads there, and reading them back
   off that same wrong engine made the mistake look like a success.

   The honest turn is short: say what you cannot do, say why, and offer what you
   can. Name the threads and let the user send the message themselves. A refusal
   reported plainly is a good turn. A refusal worked around is a broken one,
   however well it appears to have succeeded.

Three engine-side consequences worth knowing, because they change what a failed
request looks like:

- **403** on a `mode: "human"` POST means the request carried no registered
  device, so the engine will not record it as the user. Do not try to acquire
  one.
- **404** on a `thread_id` means no such thread exists *on the engine you
  reached*. It is no longer created for you. Check `GET /api/v1/health`, which
  names the workspace the answering engine serves.
- **409** naming a different workspace means you reached the wrong engine. The
  body names the right one. Several engines run on one machine, one per
  workspace, each on its own port, so never guess a port: the CLI resolves the
  target for you, and `$LUCIDOS_API_BASE_URL` is the base for this workspace's
  engine.

The CLI asserts the workspace it is talking to on every request
(`x-lucidos-target-workspace`, from `$LUCIDOS_WORKSPACE`), which is what makes
the 409 possible; `lucidos spawn-thread --to <ws>` asserts the target it was
given instead. Hand-rolled HTTP asserts nothing and is served by whichever
engine answers the port.

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

Write content into the parent workspace's `data/` tree. Creates parent dirs
automatically.

The write goes through the engine (`PUT /api/v1/data/*path`) rather than
touching the filesystem, so it needs a running engine, like `events emit` and
`notify`. That is what makes the file *arrive* in the workspace rather than just
appearing on disk: the engine commits it to the `data/` repo and announces it
(`DataFileWritten`, plus `ArtifactCreated` / `ArtifactUpdated` under
`artifacts/`), so the Files panel refreshes live, the memory index picks the
file up, an `on_event: ArtifactCreated` trigger sees it, and the chat link below
resolves. Content is limited to 100 MiB per write. A failed write is an error:
the command exits non-zero and prints no link.

Under the Codex sandbox this works without a writable-root grant, because it is
an HTTP call rather than a write outside the worktree.

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

### `lucidos data-store add <name> <source-dir>`

Move a directory to `~/.lucidos/data/<name>/` and print the absolute path.
This is the one subcommand that never talks to the engine: it is a plain
filesystem move, so it works with no workspace and no running engine.

It is for a **bulk reference corpus the user wants to keep, but not inside any
one workspace**. The store is cross-workspace and persistent, a sibling of
`~/.lucidos/knowhow/`. When to reach for it, and what belongs in
`artifacts/imported/` instead, is rule 8 in `system-knowhow/best-practices.md`.

```bash
$ lucidos data-store add sheet-music-corpus ~/Downloads/wikifonia
/Users/me/.lucidos/data/sheet-music-corpus
```

`<name>` is a single path segment. A slash, a backslash, or a leading dot is
refused. An existing destination is refused too, and the source is left
untouched, so a re-run never merges two corpora by accident. Pin the printed
path in the consuming app's knowhow, since nothing else records where it went.

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

### `lucidos events query [--type T] [--since iso] [--until iso] [--thread-id UUID] [--before-event-id UUID | --after-event-id UUID] [--limit N]`

GET events from the parent workspace's event store. Outputs the raw JSON array on stdout, newest-first.

This reads the **whole** store, not only what the workspace emitted: engine thread and system events (`ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`) are rows in the same table and come back from the same query. See `system-knowhow/thread-events.md` § "One table, two enums".

`--thread-id` narrows to one thread, which is how you READ a past conversation: pair it with `--type MessageReceived` (or `ResponseGenerated`) after finding the thread, or the query returns that thread's entire transcript including every streamed token. Omitting it queries every thread, exactly as before.

```bash
$ lucidos events query --type AnalysisCompleted --limit 1 | jq '.[0]'
{
  "id": "...",
  "event_type": "AnalysisCompleted",
  "payload": { "summary": "UA analysis for ...", "artifact": "artifacts/..." },
  "created": "2026-04-20T12:00:00Z",
  "sequence": 91240
}
```

Every row carries `id`, `event_type`, `payload`, `created` and `sequence`. A row that belongs to a thread also carries `thread_id`; domain events omit the key entirely (they are not thread-scoped).

```bash
# Engine events read the same way. thread_id here is the PARENT thread.
$ lucidos events query --type ChildThreadCompleted --limit 1 | jq '.[0] | {thread_id, status: .payload.status, child: .payload.child_thread_title}'
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

### `lucidos threads list [--active | --status <list>] [--source <list>] [--limit N] [--parent <uuid> | --my-children]`

List thread summaries from the parent workspace. Outputs the raw JSON array on stdout, newest-first by `last_activity`. Each row is a full `ThreadSummary` — the same shape returned by the `list_threads` LLM tool and by `lucidos.threads.list()` in the JS SDK, and the same shape the projection stores in `thread_summaries`.

```bash
$ lucidos threads list --status running --limit 5 | jq '.[].title'
"Plan dinner"
"Refactor settings dialog"
```

#### Picking between `--status` and `--active`

**`--status running` is what "is the workspace busy?" means.** `--active` is the **union** of `running` and `waiting_for_user_answer`, and those two are opposites: `running` is the workspace working, while `waiting_for_user_answer` is the workspace stopped and waiting on a person. A thread parked on an unanswered question is in the union while being the opposite of busy, and it is the state most likely to coincide with work piling up unnoticed, so an idle detector gated on `--active` never fires while anybody is being asked something.

- `--status <list>` restricts to exactly the statuses you name, out of `idle`, `running`, `waiting`, `waiting_for_user_answer`, `paused`, `failed`. These are the same values each returned row's `status` field carries, so you can filter on what you read; the kebab spelling `waiting-for-user-answer` is accepted too. Repeatable (`--status running --status failed`) and comma-separated (`--status running,failed`) are the same request. An unrecognized or empty value is an error listing the valid ones, never a silently empty list.
- `--active` selects the union above. Kept exactly as it was for existing callers; reach for it when you genuinely want "the loop is mid-flow in either direction", such as a badge counting threads the user has something invested in. Passing it together with `--status` is refused: they are two answers to one question.
- The `--active` union contains only those two statuses. It never contains `failed`, the response being over whether it errored or was interrupted with nobody resuming it. Nor `paused`, where the user's own version switch interrupted that turn and the engine resumes it by itself. `--status` reaches both by name, which is the only way to ask for them. It also reaches `waiting`, which nothing writes any more: it meant the coding agent had stopped with changes to review, and only older rows carry it.
- `--source` is a comma-separated list of `chat`, `trigger`, `coding-agent`. Legacy `claude_code` is also accepted. Omit for all sources.
- `--limit` clamps to `1..=1000` server-side, default 100.
- `--parent <uuid>` restricts to that thread's **direct** children only, never its grandchildren. A malformed uuid is a 400, never a silently unfiltered list.
- `--my-children` is shorthand for `--parent` with the calling thread's own id, read from `$LUCIDOS_THREAD_ID`. Use it to recover a child's `thread_id`, to see which of your children are still working, and to spot one parked on a question. Outside a Lucidos-spawned subprocess it has nothing to resolve to and errors, rather than quietly listing the whole workspace. Pass the two together and the command refuses: one filter, one answer.

```bash
# Which of my own children are still working, and what are they called?
$ lucidos threads list --my-children --status running | jq -r '.[] | "\(.status)\t\(.title)\t\(.thread_id)"'

# Which of them are stuck waiting on me?
$ lucidos threads list --my-children --status waiting_for_user_answer | jq -r '.[].title'
```

Use this from a script that needs to react to thread state — e.g. "is anything still running before I fire this trigger?" — without reconstructing it from raw `query_events`. The projection already tracks per-thread status; the list endpoint is just a read off it.

### `lucidos threads count [--active | --status <list>] [--source <list>] [--parent <uuid> | --my-children]`

Count thread summaries matching the same filters as `list`, including `--status` and the two child filters. Outputs `{"count": N}` on stdout.

```bash
# Is anything still running? (the idle-detector form)
$ if [ "$(lucidos threads count --status running | jq .count)" -eq 0 ]; then
>   echo "Workspace is idle."
> fi

# How many threads are parked on a question I have not answered?
$ lucidos threads count --status waiting_for_user_answer
{"count":1}

# The union, for a badge that counts both: working AND asking.
$ lucidos threads count --active
{"count":3}
```

Cheaper than materialising the full list just to read `.length` on big workspaces.

### `lucidos threads follow-up --thread <child-uuid> --message <M> [--event-id <E>] [--urgent]`

Send a message to one of **this thread's own child threads**: redirect one going the wrong way, hand it something a sibling learned, or tell a stalled one to continue. This is the *child follow-up* edge, the one privileged cross-thread write. Wraps `POST /api/v1/threads/<child>/follow-up`.

```bash
# Redirect the child that is taking the wrong approach.
$ lucidos threads follow-up \
    --thread 9c1f2b40-... \
    --message "Skip the CSV path entirely, the source is a live API."
{"child_thread_id":"9c1f2b40-...","child_title":"Import the sales figures",
 "delivered_to":"running",
 "detail":"The child was mid-turn, so this queues behind its current work or steers it."}
```

The ack prints as raw JSON on stdout, like every other `lucidos threads` subcommand. `child_title` is how you should refer to the child afterwards: a uuid names nothing the user can see.

Five things worth knowing:

- **You can only address your own DIRECT children.** No siblings, no grandchildren, no arbitrary thread. There is no flag for saying who you are: the engine reads the calling thread off the *thread-bound origin token* this subprocess was spawned with, then looks the relationship up from the child's own row. A thread that is not yours is a 403 whatever you claim. **This is a boundary, not an obstacle**: if you were asked to message a thread you did not spawn, there is no way to do it and no other route to try. Say so, name the threads, and let the user send it. See the prohibition at the top of this file.
- **It returns as soon as the message lands, and does not wait for the child.** The child reports back the usual way, as a completion card on its parent. The ack's `delivered_to` says which of four things happened: `running` (the child was mid-turn, so the message queues behind its work or steers it), `interrupted` (you passed `--urgent` and the child's turn is being stopped so it reads you next), `waiting-for-user-answer` (parked on a question or permission card, so **a human must answer before it reads this**), or `revived` (it was not working, so a fresh turn starts now). `detail` is the same thing in a sentence.
- **`--urgent` is for cancellations, not for hurry.** By default a mid-turn child reads you when its current work reaches a natural break, and if it is inside a long tool call that can be many minutes: a coding-agent child parked in a ten-minute blocking wait reads you when that wait returns, not before. `--urgent` stops the child's current turn instead, so it reads you at once, and whatever that turn was mid-way through is lost. Use it when the child must act on your message *instead* of what it is doing. Use the default for anything you would be happy for it to read later.
- **Address the child by uuid, never by title.** Titles are not unique, and a fuzzy match would silently deliver to the wrong child. Find the id with `lucidos threads list --my-children`. The ack carries `child_title`, so refer to the child by that afterwards rather than by the uuid you typed.
- **A follow-up consumes no child slot.** The fan-out limit counts threads spawned, not messages sent, so reviving a child you already have is cheaper than spawning another one.

`--event-id` defaults from `$LUCIDOS_EVENT_ID` and stamps the child's message-route panel so the follow-up links back to the originating event.

**A cancellation is not done when the ack returns.** The ack says the message is on the child's timeline, nothing more: even with `--urgent` the child still has to pick it up, read it, and do the work of stopping. If you told a child to kill a running job, verify the job is actually gone (no processes, no lock file) before you report the cancellation as complete. Reporting off the ack is how a nightly pipeline once announced a clean host while its e2e suite ran on for another seven minutes.

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

### `lucidos await-event --on <EventType> [--on <EventType> ...] [--condition <JSON>] --timeout-secs <N> --reason <R>`

Subscribe the **calling thread** to a Lucidos event, then finish. The engine
re-opens the thread with a follow-up message when a matching event lands, or
tells it the deadline passed. The coding-agent counterpart of the chat agent's
`await_event` tool, on the same registration underneath, so both agents get the
same caps and the same refusals.

**It returns immediately and blocks nothing.** The thread is plain **idle** while
it holds a subscription: no queue slot, no running turn, nothing for the user to
resolve. So the correct shape is *subscribe, say what you are waiting for, end
the session*. Sitting in a sleep-and-recheck loop afterwards is the thing this
replaces, and polling for the event as well is strictly worse than either.

Reach for it whenever the thing you are waiting on is something the engine
emits: a change appearing (`ChangeProposed`), a trigger firing
(`TriggerExecuted`), a backup finishing (`BackupCompleted` / `BackupFailed`), a
workspace domain event your own scripts emit. Any persisted event works, and a
transient frame such as `BackupProgress` is refused by name. It is
**not** for external state with no Lucidos event (a third-party API you can only
re-query, a file another process may write): nothing would ever be delivered, so
poll for those.

**And never subscribe to your own child's completion.** A thread spawned with
`lucidos spawn-thread --relation child` already re-opens this one: when it finishes, the
engine emits `ChildThreadCompleted` here and re-opens this thread with the
child's status, summary and `pending_change_ids`, which is everything a
subscription would have handed you. So a wait on it buys nothing, and it costs
two things: one of the consecutive subscriptions the loop cap below allows, and
a second clock, since
a child that outlives `--timeout-secs` re-opens this thread with a pointless expiry
and then re-opens it again when it actually finishes. Await a `ChildThreadCompleted`
only for a completion that is **not** your own child's, named with
`--condition '{"child_thread_id": "<uuid>"}'`. Matching is workspace-wide, so
that is any thread's child and not only a descendant of yours: a coding-agent
session another thread spawned is a first-class thing to watch, and the wait
fires when its completion lands on that thread. A session **nobody** spawned
(the user started it themselves) has no `ChildThreadCompleted` at all, since
only the parent/child fan-in emits one. Watch its turn boundary instead:
`--on CodingAgentIdled --condition '{"thread_id": "<uuid>"}'`.

A **rendezvous, not a stream**. The first match resolves the subscription and
consumes it. "Continue when the next X happens" is this; "react to every X,
forever" is a *trigger*.

**It watches forward only, so still check whether it already happened.** A
subscription cannot fire for an event that has already gone by, so if the thing
might be in the past, look at state first as you would anyway. What you do not
have to worry about is the race between that check and this command: if a match
landed in the few minutes just before it, the response names it, with its age.
Read that part rather than skimming to the `"status":"subscribed"`, and act on
it before you finish, because nothing will deliver it to you. It is a report and
not a delivery because only you can tell an event you missed from one you handled
yourself a few minutes ago.

- `--on` names the event type, PascalCase past tense. Repeat it to watch
  several: any one of them re-opens the thread.
- `--condition` is a JSON object filtering the event's OWN payload by field
  path (a dot reads one level down), applied to every `--on` name. Equality by
  default, or an operator object: `{"$eq":v}`, `{"$ne":v}`, `{"$lt":n}`,
  `{"$lte":n}`, `{"$gt":n}`, `{"$gte":n}`, `{"$in":[…]}`, `{"$nin":[…]}`,
  `{"$regex":"…"}`. `$or` in key position takes a list of whole conditions;
  see `system-knowhow/triggers.md` § "What a condition can say" for the full
  language. One field beyond the payload is always filterable on a thread
  event: `thread_id`, supplied by the engine from the thread the event
  belongs to, so
  `--condition '{"thread_id":"<uuid>"}'` scopes the wait to one thread. It will
  not appear in what `lucidos events query` prints (the event row carries the
  thread in its own column), and a **domain event** belongs to no thread and so
  has none to filter on.
- `--timeout-secs` is required and capped at 86400 (24 h). There is no unbounded
  subscription. Giving up early costs one turn; giving up too late costs the
  user the whole wait.
- `--reason` is one short line in the user's language, naming **what** you await
  rather than the fact that you await it. They read it in the subscription
  indicator, and it is how they tell a sleeping thread from a stalled one.
  Write `"the e2e lock to free up"`, not `"waiting for the e2e lock"`: the
  transcript labels it `Set up an event wait: <reason>`, so a reason opening
  with a waiting word says it twice.

Refusals arrive as a `400` carrying the reason, and are worth reading rather
than retrying: a per-token streaming event (`TextStreamed` and friends) or an
`EventWait*` type is refused outright, a thread may hold at most 25 live
subscriptions, the same `--on` list twice on one thread is refused (it would
deliver one event to you twice), and 10 subscriptions in a row with no message
from the user is the loop cap.

```bash
# Wait for a domain event the workspace's own scripts emit, then stop. The
# engine re-opens this thread with the payload when it lands.
$ lucidos await-event --on E2ETestsPassed --timeout-secs 3600 \
    --reason "tonight's e2e run to report"

# Narrow it: only a change that actually touched files.
$ lucidos await-event --on ChangeProposed --condition '{"file_count": {"$gt": 0}}' \
    --timeout-secs 1800 --reason "the refactor to propose its change"
```

### `lucidos build-slot [--label <T>] [--max-wait <SECS>] -- <command>` / `--status` / `--set-capacity <N>`

Run a heavy build under a *build slot*, so parallel *worktrees* cannot pile N
full compiles onto one host and OOM it.

**Wrap anything heavy** that a coding-agent session runs: `cargo build`,
`cargo test`, a Gradle or Xcode build, a large bundler run. The slot is taken
before the command starts and freed when it exits, or when this process dies.
Do NOT wrap cheap work (a type-check, a unit-test run of a small package):
it would sit in a slot for minutes to save seconds.

```bash
# The normal shape. Blocks until a slot frees, then runs the build.
$ lucidos build-slot -- cargo test --release

# Name it for the listing, when the command line is not the useful label.
$ lucidos build-slot --label "integration suite" -- ./gradlew test

# Who is building right now, and where the count came from.
$ lucidos build-slot --status
build slots: 1/3 held, capacity from host RAM
pool: /Users/me/.lucidos/build-slots
  slot 0  HELD  cargo test --release  pid 41231  2m14s  /path/to/worktree
  slot 1  free
  slot 2  free

# Set the count for this machine. Not per workspace: the pool spans them.
$ lucidos build-slot --set-capacity 2
```

**In the Lucidos repo you do not need this.** `make lint`, `make test` and the
build scripts already take a slot themselves, so type the ordinary command.
Reach for the wrapper in any OTHER repo, where nothing wraps it for you.

**It waits, it does not fail.** A second build is wanted, just not
concurrently, so the loser blocks with no deadline and prints progress.
`--max-wait <secs>` opts into a deadline and exits **75** when it passes. If
you set one and hit it, do not retry on a timer: subscribe and end your turn.

```bash
$ lucidos await-event --on BuildSlotReleased --timeout-secs 3600 \
    --reason "a build slot before running the test suite"
```

Notes that matter:

- **Nesting is safe.** A wrapped command that wraps again runs straight
  through, so a script you call cannot deadlock against the slot you hold.
- **It never blocks a build it cannot govern.** No `lucidos` binary, no
  writable pool, or no engine to announce to all mean the command just runs.
- **The exit code is the command's**, and a signalled command reports
  `128 + signal`, so a killed build never reads as a pass.
- **Every release is announced.** `BuildSlotReleased` fires whenever a slot
  frees, so a session that gave up on `--max-wait` and subscribed is always
  woken. `BuildSlotWaiting` and `BuildSlotAcquired` fire only under contention.

### `lucidos event-waits list` / `lucidos event-waits cancel [--wait-id <ID>] [--on <EVENT_TYPE>] [--all]`

Read and stop the **calling thread's** own subscriptions, the ones
`lucidos await-event` armed. The coding-agent counterparts of the chat agent's
`list_event_waits` and `cancel_event_wait` tools, on the same code underneath,
so both agents get the same report and the same refusals. Like `await-event`,
both take the thread from `$LUCIDOS_THREAD_ID` and have no thread flag, so
neither can reach another thread's subscriptions.

**`list` is how you answer "am I still watching for that?", and you cannot
answer it any other way.** Nothing tells you when a subscription ends. A
delivery re-opens the thread, but a timeout or a user pressing **Stop waiting** lands
while your session is not running, and a subscription is *spent* the moment it
fires. Answering from memory is a guess: on 2026-08-06 a thread told its user
twice that a watch was armed when it had been dead for two hours. Run it before
saying you are still watching, before re-subscribing to something you may
already be watching (a duplicate is refused), and to get the id `cancel` takes.

Each entry carries the subscription's id, the events and conditions it watches,
the `--reason` it was armed with, when it was armed, and when it times out, with
both ages spelled out:

```bash
$ lucidos event-waits list
{"count":1,"event_waits":[{"wait_id":"3f2b…","subscription":"ChangeProposed",
  "reason":"the refactor to propose its change",
  "armed_at":"2026-08-07T09:14:22Z","armed_ago":"7m",
  "expires_at":"2026-08-07T09:44:22Z","expires_in":"22m"}]}
```

**`cancel` is how you stop watching.** A subscription you leave live re-opens this
thread later whatever you told the user, so when they say to stop, drop it, or
never mind, run this rather than promising. Use it too when the thing turns out
to have already happened, or when a new subscription supersedes an old one.

Pass exactly one of `--wait-id <ID>` (from `list`), `--on <EVENT_TYPE>`, or
`--all`. None is defaulted: a bare call would have to guess between stopping one
and stopping every one, and both guesses are wrong. Stopping is silent, so
nothing interrupts you: the subscription simply ends, the user sees it leave the
waiting indicator, and the transcript records the stop.

**`--on` is the one to reach for when the answer arrived some other way**, and
it is the safe middle: it needs no id, so nothing has to be read out of `list`
first, and it leaves every other watch on this thread standing, which `--all`
does not. It ends every subscription watching that event type, whatever
`--condition` each one carries.

One sharp edge, because a subscription can watch several event types at once
(repeated `--on` at `await-event`): naming ONE of them ends that whole
subscription, the other names included. A wait is a single rendezvous with
several triggers, spent by the first match, so there is no leg left to deliver
once you have stopped watching for the other. The result names every type it
ended, so read it rather than assuming; when you meant to keep watching for the
rest, arm a new subscription for them.

```bash
# The user changed their mind about one of several watches.
$ lucidos event-waits cancel --wait-id 3f2b1c04-...
{"status":"stopped","message":"Stopped watching for ChangeProposed. It will not re-open this thread."}

# The release build finished while you were doing something else.
$ lucidos event-waits cancel --on ReleasePublished
{"status":"stopped","message":"Stopped watching for ReleasePublished. Nothing on this thread watches ReleasePublished any more. 1 other subscription(s) on this thread is still live."}

# Stand everything down.
$ lucidos event-waits cancel --all
```

Refusals arrive as a `400` carrying the reason: more than one flag or none of
them; a `--wait-id` that is not live on this thread (already fired, timed out,
already stopped, or belonging to another thread, which are indistinguishable and
equally mean "not yours to stop"); and an `--on` nothing on this thread is
watching, which is worth reading rather than shrugging off, because it means a
watch you thought was armed is not.

Scripts use this too, and `scripts/lib/e2e_lock.sh` is the worked example: the
moment a run takes the machine-wide e2e lock it runs
`lucidos event-waits cancel --on E2ELockReleased`, because holding the lock is
the answer to any watch this thread had for its release. That call is best
effort and its refusal is discarded, since most runs never subscribed.

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

The `TRIGGER_EVENT_THREAD_ID` and `TRIGGER_EVENT_ID` env vars in the snippet are set by the engine on every script trigger fired by a thread-scoped event (see `triggers.md` § "Script trigger env vars"). For schedule-fired triggers neither is set, so `--tap modal` (the default) is the only meaningful choice.

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

### `lucidos triggers list | create | update | delete | run`

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
# Fire an existing trigger once, right now, outside its schedule
$ lucidos triggers run --id <uuid>
# Pin an intent trigger to its own model and thinking budget
$ lucidos triggers update --id <uuid> --model gemini-3.5-flash --reasoning-effort low
```

`--cron-expressions` entries are validated on `create` and `update`. Within one
expression the fields are ANDed and across the array they are ORed, so
`0 0 9 1 * Mon` is the 1st only when it is a Monday. An expression that can
**never** fire (`0 0 9 31 2 *`, Feb 31) is refused with an error naming the
offending fields, and a successful write returns a `cron_preview` object
carrying `next_runs` (the next few fire times) plus any `warnings`. Read the
preview back rather than assuming the schedule means what you intended;
`system-knowhow/triggers.md` § "Writing cron expressions" has the recipes.

`create`/`update` accept `--name`, `--run`, `--cron-expressions`, `--on`,
`--app-id`, `--go-to-review`, `--group-id`, `--side-effect-grant`, `--slug`,
`--model`, `--reasoning-effort`;
`update`/`delete`/`run` take `--id <uuid>`. The chat agent's in-process
equivalent is the grouped `triggers` tool (`action: create | list | update |
delete | pause | resume | run`). Pause/resume are tool-only there; the CLI
pauses via `update --paused`.

`run` performs an **off-schedule run**: a real fire that records
`TriggerExecuted` / `last_run` and carries the trigger's own identity,
side-effect grant and `go_to_review`, indistinguishable downstream from a
scheduled one. It returns as soon as the run is admitted, so a `success: true`
response does not mean the work finished. Read `status` in the response body:
`started`, `queued` (over capacity), or `already-running` (a fire was already
active or queued, so **nothing new started**). It is refused for a paused
trigger and for an event-only one (emit its subscribed event with
`lucidos events emit` instead); the `message` field says which.

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

### `lucidos memory stats | entries [...] | search --q <Q> [--limit N] | source [--source-id UUID] [--source-type T] [--path P] [--commit C]`

Read long-term memory. `stats` (index counts), `entries` (paginated, with
importance and source), `search` (rank entries against a question), `source`
(the originating event or artifact for one memory, plus the entries derived
from it). All read-only.

```bash
$ lucidos memory stats
$ lucidos memory entries --limit 20 --importance high,critical
$ lucidos memory search --q "launch outcome" --limit 5
$ lucidos memory source --source-id <uuid>
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
it does not have is `stats` and `entries`, which are operator reads.

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

`provider` is one of `vertex`, `anthropic`, `openai`, `openrouter`, `xai`,
`opencode-free`, `local`.

**`--context-window` is worth setting on every model you add.** It's the model's
context window in tokens, and it sizes the engine's context budget. Omit it and
the engine falls back to guessing from the model id (`claude-*` → 200k unless the
id carries `[1m]`, `gpt-5*` → 400k, anything else → 200k). That guess has no rule
at all for OpenRouter, xAI, Gemini, or local ids, so they are treated as 200k however
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

### `lucidos mcp list | start --id <id> | stop --id <id> | remove --id <id>`

Manage MCP servers: which are running, what tools they offer, and what those
tools cost in context.

```bash
$ lucidos mcp list
$ lucidos mcp start --id slack
$ lucidos mcp stop --id slack
$ lucidos mcp remove --id backstage
```

`list` returns `servers`, `totals`, `model` and `context_window`. Every server
carries its tools with a `wire_name` (the name a call must use), `chars` and
`tokens`. The token figures are the engine's own estimate, the same one the
Context Viewer shows, so a script must never recompute them from chars.

`tools_source` says where the tool list came from. `live` means the process
answered just now. `cache` means the manifest observed at the last successful
start, and `tools_observed_at` stamps when. `never-observed` means the server
has never connected, so its tool list is unknown: that is NOT the same as a
server with no tools, and a script must not report it as costing nothing.

`totals` splits the cost. `tokens` is what running servers add to every request
right now. `stopped_tokens` is what the stopped ones would add if started, and
`disabled_tokens` what the switched-off tools would add back. Divide by
`context_window` for the share of the resolved model's window.

**Nothing starts MCP servers at boot.** A server is running only if something
started it in the current engine process, so `running` resets on every restart.

A server whose id cannot ride a wire tool name reports `dispatchable: false`.
None of its tools can ever be called, `start` refuses it with a 422, and
`remove` is the only useful verb. Registering a server is the chat agent's `mcp`
tool, which takes the command and args this surface does not.

Switching individual tools off is `PUT /api/v1/mcp/servers/<id>/disabled-tools`
with `{"disabled_tools": ["<wire name>", ...]}`, a full replacement rather than a
delta. No CLI flag for it: the set is a selection, not a scalar.

### `lucidos changes list`

List pending and recently-applied *changes*. Wraps `GET /api/v1/changes` and echoes the engine's payload verbatim to stdout. This is the canonical way for a script to find a pending change's id before `apply` — read `.pending[].id`. Don't scan `ChangeProposed` events for the id when this one command gives it directly.

```bash
$ lucidos changes list
{"pending":[{"id":"fbcc4a3a-...","branch_name":"lucidos-claude-code-repo-lucidos-fix-...","description":"fix: …","status":"pending",...}],"applied":[...],"total_pending":1,"restart_required":false,"restart_groups":[],"client_update_available":false,"has_more_applied":false}

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

#### Why the CLI and not hand-rolled urllib / curl

Because a hand-rolled request gets two things wrong that you cannot see from
inside the script, and because of the rule at the top of this file: do not reach
for raw HTTP to the engine at all.

**It loses your identity.** The CLI auto-forwards the subprocess-origin header
(`x-lucidos-agent-origin-token`) that the engine reads to stamp the resulting
`ChangeApplied` event as `Api { mode: Agent, source_thread_id }`. The token is
*thread-bound*: the engine mints one per spawn and reads the spawning thread off
the token itself, so one header carries both facts, and the popover links back
to the thread that acted. Without it the engine sees an unattributed API client:
your agent action is recorded as an anonymous one, and on the chat path it is
refused outright. A `run_python` block calling
`urllib.request.urlopen(".../api/v1/changes/<id>/apply")` hits this, because
urllib does not read the env var on its own.

**It can reach the wrong engine.** The CLI resolves this workspace's engine and
asserts which workspace it is talking to, so a mis-resolved port comes back as a
409 naming the right one. A hand-built `https://localhost:<port>/...` asserts
nothing and is served in full by whichever engine happens to hold that port,
which is how six threads were created in an unrelated workspace on 2026-08-06.

```python
# WRONG: unattributed, and aimed at a port you guessed
import ssl, urllib.request as r
ctx = ssl._create_unverified_context()  # self-signed cert
r.urlopen(r.Request(f"https://localhost:{port}/api/v1/changes/{cid}/apply", method="POST"), context=ctx)

# RIGHT: the CLI forwards the headers and resolves the engine, so the UI says
# "Lucidos Agent" with the source thread linked
import subprocess
subprocess.run(["lucidos", "changes", "apply", cid], check=True)
```

The same rule applies to bash:

```bash
# WRONG: bare curl from inside a run_bash tool
curl -k -X POST "https://localhost:$LUCIDOS_API_PORT/api/v1/changes/$CID/apply"

# RIGHT: the CLI handles the headers and the target
lucidos changes apply "$CID"
```

**If there is no CLI subcommand for what you want, that is the answer.** Do not
substitute raw HTTP: read the top of this file. The only callers that
legitimately speak to `/api/v1` directly are test harnesses and external tools
that cannot shell out, and neither of those is an agent working around a
refusal.

Where the CLI does cover the operation and you need the underlying URL for some
other reason, use `$LUCIDOS_API_BASE_URL` (set by the engine on every spawned
subprocess) rather than building one from `$LUCIDOS_API_PORT`: under the
workspace gateway (ADR 0014) the engine binds a **loopback HTTP** port while the
user-facing port belongs to the gateway, which routes the workspace under
`/<slug>/`, so a bare `https://localhost:$LUCIDOS_API_PORT/api/v1/...` request
there never reaches the engine (the gateway resolves the first path segment as a
workspace slug). `$LUCIDOS_API_BASE_URL` is the exact base this engine answers
on: loopback `http://` under the gateway, `https://` self-signed in the legacy
single-engine model. See `docs/apply-change-api.md` for the apply response shape
and the full workflow.

### `lucidos planned mark (--plan <path> | --simple "<reason>")` / `lucidos planned approve` / `lucidos planned state`

Record, approve, or query the *plan marker* — the durable enforcement that the `implementation-plan` skill ran AND the human approved its plan (or that a local fix was acknowledged) before a *Lucidos-source* coding-agent branch is edited and applied. A **gate-satisfying** marker MUST exist on the branch or Claude Code's first source edit is blocked (the `cc-plan-gate` PreToolUse hook) and Apply is refused (the engine's plan floor). Wraps `POST /api/v1/internal/mark-planned` / `POST /api/v1/internal/approve-plan` / `GET /api/v1/internal/planned-state`.

```bash
# Complex work: the implementation-plan skill writes the plan, then records this for you.
# This records the AWAITING-APPROVAL `proposed` state — it does NOT unblock editing:
lucidos planned mark --plan docs/plans/2026-06-19-my-change.md

# Present the plan, then ask for approval with your question tool (AskUserQuestion on
# Claude Code, ask_user_question on Codex; options `Approve` / `Request changes`), not in
# prose. That pair is a floor: if the plan has a real fork, the fork takes the second
# slot and `Request changes` is dropped. Once the user APPROVES, flip it to gate-satisfying:
lucidos planned approve

# Genuinely local fix that doesn't warrant a plan — acknowledge instead (no approval needed):
lucidos planned mark --simple "rename a misspelled variable"

# Inspect the current branch's marker (SATISFIED, PROPOSED, or MISSING):
lucidos planned state
```

`mark` / `approve` resolve repo_root / branch / HEAD from `$PWD`'s git worktree (like `lucidos hardened mark`). Pass exactly one of `--plan` / `--simple`. **`mark --plan` records `proposed` (awaiting approval); it does NOT satisfy the gate.** The agent must present the plan and ask for approval **with its question tool** (`AskUserQuestion` on Claude Code, `ask_user_question` on Codex; options `Approve` / `Request changes`), never in prose: approval is a DECISION question the agent is blocked on, and asked in prose it leaves the thread idle until the user types "approve" by hand. That option pair is a **floor**, not a fixed shape: the question tool needs at least two options, so `Request changes` fills the second slot only when the plan offers no real fork. When it offers one (a narrower scope, one layer instead of two), that fork takes the slot and `Request changes` is dropped rather than carried as a third option, where it would mean only "I will type what I want changed". Only after the user approves does the agent run `lucidos planned approve` to flip `proposed`→`planned` (gate-satisfying); a fork answer is an approval too, so the agent revises the plan file to that variant, re-commits, and then flips it. If the user requests changes instead, revise the plan file, re-commit, and ask again the same way (the marker stays `proposed`). `mark --simple` records `acknowledged_simple` directly, since local fixes need no approval. `planned` and `acknowledged_simple` satisfy every gate; `proposed` and the absence of a marker both block. App coding-agent threads and external repos are exempt (the gate is a no-op there). Normally you don't call `mark --plan` / `approve` by hand (the `implementation-plan` skill drives them), but `mark --simple` is the agent's escape hatch for a change too small to plan. (`lucidos cc-plan-gate` is the hidden PreToolUse hook that enforces this; it is not invoked directly.)

### `lucidos frontend-preview start [--thread-id <uuid>]` / `lucidos frontend-preview stop` / `lucidos frontend-preview status`

Start, stop, or inspect the **frontend preview**: a Vite dev server the engine supervises inside a coding-agent worktree, on its own port, so a TypeScript or CSS change is visible in the real app **before Apply**. Wraps `POST /api/v1/frontend-preview/start` / `/stop` and `GET /api/v1/frontend-preview`. Development only, and refused on a packaged install.

```bash
# Inside a coding-agent worktree: preview THIS thread's branch. --thread-id
# defaults to $LUCIDOS_THREAD_ID, which every coding-agent subprocess carries.
lucidos frontend-preview start
# → Frontend preview running for thread <uuid> at https://localhost:6173/

# Point it at a different thread's worktree (there is one slot, so this replaces
# whatever was running).
lucidos frontend-preview start --thread-id 2951200f-0652-4ee2-baa3-433d608983d8

lucidos frontend-preview status
lucidos frontend-preview stop
```

**Why the CLI rather than starting `vite` yourself:** when a coding-agent turn ends, the engine kills the session's whole process group, so a dev server the agent started dies with the message. The engine owns the process precisely so the preview outlives the turn and the user can look at it afterwards.

**One slot per workspace.** `start` on another thread moves the preview rather than adding a second one.

`start` refuses, by name, when the thread has no worktree, when the worktree is not a Lucidos-source one (an app or external-repo thread has no frontend to preview), or when its dependencies were never provisioned. It answers only once Vite is actually serving, so the printed URL is live when you paste it into a reply.

**The printed URL uses the host the CLI reached the engine on**, which from inside the worktree is `localhost`. That is right for the host machine and wrong for a phone: the engine builds the URL from the caller's `Host`, and the in-app control (the coding-agent control menu's *Frontend preview* section) builds it from the page's own location instead, so a user on a tailnet gets a link that resolves. Prefer pointing the user at that control over pasting a `localhost` URL. It also carries the device id, which the CLI has no way to know, and without it the preview renders with none of that device's scoped preferences.

The preview registers **no service worker** and cannot do push: a dev server emits unhashed module URLs a worker would cache past a hot update. See ADR 0055 for the whole design.

### `lucidos knowhow list`

List the merged user + system-knowhow catalog. Wraps `GET /api/v1/knowhow` and echoes the engine's payload verbatim: `{ "knowhow": [{ "id", "name", "description" }] }`. Engine-shipped reference docs carry the `system-knowhow/` id prefix; user-curated knowhow uses its path under `data/knowhow/` without `.md`. Read `.knowhow[].id` to find the id to pass to `read`.

The catalog holds knowhow *docs*. A doc's own reference files sit below the listed depth, so `list` does not show them and `read` still takes their full id. See `system-knowhow/building-knowhow.md` § "Where the file goes".

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
lucidos proxy sonos /living-room/play

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
| Emit a domain event, or query the event store (domain AND engine events) | `lucidos events …` |
| Write a file under `data/` | `lucidos data write …` |
| Push a notification to the user from a script | `lucidos notify --title … --message …` |
| Find a pending change's id from a script | `lucidos changes list` (read `.pending[].id`; don't scan `ChangeProposed` events) |
| Apply a coding-agent-proposed change from a script | `lucidos changes apply <id>` (never hand-roll the HTTP call — actor stamps as "You") |

If you find a script doing `curl -H "Authorization: Bearer $CRED_..."` against an API the workspace already owns a credential for, that's drift — add an `apis.json` entry and switch the script to `lucidos proxy`.

### `lucidos pair` (mint a code that lets a device in)

Lucidos authenticates every caller that reaches it over the network. A device is
paired once and then remembered, so a stranger who reaches the port is refused.

```bash
lucidos pair                        # print a code, and where to enter it.
lucidos pair --qr                   # draw it as a QR to scan.
lucidos pair --host mac.ts.net      # pick the hostname the QR points at.
lucidos pair --label "My iPhone"    # name the device in the paired list.
lucidos pair --port 5300            # a gateway on an unusual port.
```

Run it in a terminal on the machine Lucidos runs on, then type the code into the
device you want to let in. It works once and expires in five minutes.

**With two gateways running it refuses rather than guessing.** A device pairs to
a gateway, so a code only works on the one that minted it (ADR 0132). This
command finds a gateway by probing 5252 then 5251, which is the packaged app
then a dev checkout. Both answering is ambiguous, so it stops and names the
ports. Pass `--port` to say which one the new device will reach.

**Reach for it only when nothing is paired yet, and know where it lives.** The
desktop app pairs its own window on launch. Any paired device can add the next
one from **Settings → Access → Add a device**. `lucidos` is on no `PATH`
either: a desktop install keeps it at `Lucidos.app/Contents/Resources/lucidos`,
and a headless one under the install prefix in `runtime/current/`.

**`--qr` needs an address the phone can reach.** This command talks to
`127.0.0.1`, and a QR aimed there helps nobody. So it resolves a hostname from
the interface list: the MagicDNS name, else the tailnet address, else whatever
`--host` says (which implies `--qr`). Tailscale is only picking a name to
print, and the auth decision reads none of it.

**Then it knocks on the door.** Holding a tailnet address is not the same as
being reachable at it, and both defaults get that wrong in opposite
directions. The packaged gateway binds loopback, where `<name>:<port>` is
dead. `tailscale serve` fronts 443 on that same name and answers where the
gateway's own port does not.

So both origins are probed, and the first that answers wins. With neither
answering there is no QR, and the command says which knob to turn. An explicit
`--host` is never refused: probing then only picks which of its two origins to
use.

The QR is drawn black on white. A dark terminal would otherwise invert it, and
many scanners refuse that. `NO_COLOR` drops the escapes.

**From an already-paired device there is no terminal step at all.** Settings →
Access → Add a device mints the same code and shows the same QR.

**A browser has to pair too, even on that same machine.** Proving you are local
means reading a file only your user can read, and a browser cannot read files.
So a browser on the host goes through the same code as a phone does.

Only a process on that machine can mint a code, which is what stops a remote
caller from pairing itself in. An already-paired device may also mint one, so
you can add a tablet without walking back to your desk.

Nothing you run from a coding-agent session, a trigger or a script needs this.
Those already prove they are local, and the CLI attaches that proof itself.

### `lucidos webhooks list | create | update --id <id> | delete --id <id>`

An endpoint a third party posts to, emitting one **pinned** domain event that a
trigger can react to. The event is fixed when you create the webhook, so an
endpoint you gave GitHub can only ever fire that one event.

```bash
lucidos webhooks list
lucidos webhooks create --name deploys --event-type DeployFinished
lucidos webhooks update --id <uuid> --enabled false
lucidos webhooks delete --id <uuid>
```

`create` prints the webhook plus a **token**, and that is the only time the
token exists in readable form. Only its digest is stored. A sender presents it
as `Authorization: Bearer <token>`.

**A signed webhook gets no token**, and that is what makes it usable. GitHub
cannot attach one, so a hook holding both verifiers would refuse every real
delivery. Configure `--hmac` and the hook authenticates by signature alone.

Deliveries go to `{host}:{hook_port}/<slug>/<webhook-id>`, on the gateway's
*hook socket* rather than its main port. `list` prints the path half of that as
`delivery_path`; the host and port are your own. The hook port is the gateway's
plus ten, so 5261 in dev and 5262 packaged.

GitHub, Stripe and Slack authenticate by signing the request body with a shared
secret. Save that secret as a credential, then name it in `--hmac`:

```bash
lucidos webhooks create --name github --event-type PullRequestOpened \
  --hmac '{"credential":"example-repo-webhook",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}'
```

The secret stays in the credential; the webhook holds only its name. Slack adds
`"timestamp_header":"X-Slack-Request-Timestamp"` with
`"template":"v0:{timestamp}:{body}"` and `"prefix":"v0="`. Stripe packs both
fields into one header, so it takes `"signature_key":"v1"` and
`"timestamp_key":"t"` with `"template":"{timestamp}.{body}"`.

A webhook needs at least one verifier and every one it has must pass. There is
no LLM tool and no SDK namespace for any of this, deliberately: a webhook opens
a publicly reachable door, so only you create one.

#### What a delivery becomes

Always three keys: `{summary, headers, payload}`. The sender's body is under
`payload`, so a trigger condition reads `payload.action`. `headers` holds the
request headers you allow-listed, read as `headers.X-GitHub-Event`. `summary` is
the sender's own if the body has one, and a generated line otherwise.

`--headers` is that allow-list. Without it the map is empty:

```bash
lucidos webhooks update --id <uuid> --headers '["X-GitHub-Event"]'
```

**`Authorization` and the hook's own signature header are refused**, since the
event log is append-only and a carried secret would stay on it for good.

#### Deduping a resend

Senders resend. GitHub retries a slow response and has a Redeliver button,
Stripe retries for days. By default Lucidos emits on every arrival, so a resend
fires your triggers twice, and the log shows you how often it happens.

`--dedupe` opts out of that. Name the header carrying the sender's own delivery
id, and a resend inside the window emits nothing:

```bash
lucidos webhooks update --id <uuid> \
  --dedupe '{"header":"X-GitHub-Delivery","window_secs":3600}'
```

The resend answers 200 with `"duplicate": true` and the event id the first
delivery emitted, so the sender stops retrying. A resend that lands while the
first delivery is still being handled gets a 503 instead: that one can still
fail, and telling the sender "done" would lose the delivery. Omit `header`
and the key is a digest of the body, which collapses two identical bodies inside
the window. `window_secs` defaults to an hour and is capped at seven days;
`0` switches deduping back off.

Leaving it off is a real choice, not just the lazy one. Every arrival stays on
the log, so allow-list the delivery-id header and a script trigger can count how
often a sender resends.

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
