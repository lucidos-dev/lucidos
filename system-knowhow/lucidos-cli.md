---
name: Lucidos CLI (`lucidos`)
description: The `lucidos` shell command, on PATH in every subprocess Lucidos spawns (Python, bash, Claude Code, Codex). It writes files under data/, emits and queries domain events, and awaits an event instead of polling. It applies pending changes and calls an external API through the engine proxy. It spawns threads, in this workspace or another (`spawn-thread --to <ws>`).
---

# `lucidos` CLI

A shell command on the `PATH` of every subprocess Lucidos spawns: Python scripts, bash scripts, and coding-agent sessions (Claude Code or Codex). Use it when a script needs to:

- write files into the workspace's `data/` directory
- emit a domain event, or query the event store (`events query` returns engine thread and system events as well as domain events)
- list or count *thread summaries*, for "is anything still running?" gates in triggers
- spawn a new *thread* with `lucidos spawn-thread`: a chat thread, or a *coding-agent thread* on a repo or an app folder. `--coding-agent claude-code|codex` picks the backend, `--folder data/apps/<id>` targets an app worktree
- subscribe the calling thread to an event instead of polling, and finish. The engine re-opens the thread when the event lands: `lucidos await-event`
- read what this thread is subscribed to, and stop watching: `lucidos event-waits list` / `lucidos event-waits cancel`
- run work that has to outlive a coding agent's turn, and be re-opened when it finishes: `lucidos background-task run --description "<what it is>" -- <command>`
- list pending and applied *changes* (`lucidos changes list`), and apply a pending one: `lucidos changes apply <id>`
- read system-knowhow and user knowhow: `lucidos knowhow list` / `lucidos knowhow read <id>`. An *app coding-agent thread* cannot see `system-knowhow/`, so this is how it pulls app-building guides
- call an external API configured in `data/config/apis.json`. The engine injects the auth header, so the credential never appears in the script
- send a push notification to the user without an LLM thread

The CLI is a thin Rust wrapper around the engine's HTTP API and filesystem conventions. For app UIs, see the JS [`lucidos.data.*`](./js-sdk.md) reference. Scripts always prefer the CLI over hand-rolled HTTP calls to the engine.

## Never post to the engine API as the user, and never route around a tool

Read this before anything below it. Three rules, and the third matters most.

1. **Never fabricate a human turn.** A message you author is an *agent* message.
   Never POST `mode: "human"` to `/api/v1/chat/stream` (or anywhere else). A turn
   recorded as the user looks, in the timeline and the event log, exactly like
   something they typed. It also sets the thread's initiator to the user and
   bumps the drawer's recency sort. The engine refuses a human claim it has no
   evidence for, but the rule binds you whether or not the engine catches you.

2. **Never hand-roll HTTP to the engine to get past a restriction.** If a tool
   or a CLI subcommand will not let you do something, that is the answer. Do not
   reach for `curl`, `urllib` or `fetch` against the engine's own `/api/v1`
   surface to do it another way. Use the CLI (it forwards the attribution
   headers and resolves the right engine) and stop where it stops.

3. **When a tool refuses you, TELL THE USER IT IS NOT POSSIBLE.** An agent once
   curled the engine to message threads that were not its children. Six agent
   messages were recorded as the user's, on another workspace's engine, as six
   phantom threads.

   The honest turn is short: say what you cannot do, say why, and offer what you
   can. Name the threads and let the user send the message. A refusal reported
   plainly is a good turn. A refusal worked around is a broken one, however well
   it seems to succeed.

What a failed request looks like:

- **403** on a `mode: "human"` POST means the request carried no registered
  device, so the engine will not record it as the user. Do not try to acquire
  one.
- **404** on a `thread_id` means no such thread exists *on the engine you
  reached*. It is no longer created for you. Check `GET /api/v1/health`, which
  names the workspace the answering engine serves.
- **409** naming a different workspace means you reached the wrong engine. The
  body names the right one. One machine runs one engine per workspace, each on
  its own port, so never guess a port. The CLI resolves the target, and
  `$LUCIDOS_API_BASE_URL` is the base for this workspace's engine.

The CLI asserts its workspace on every request (`x-lucidos-target-workspace`,
from `$LUCIDOS_WORKSPACE`), which makes the 409 possible.
`lucidos spawn-thread --to <ws>` asserts its given target instead. Hand-rolled
HTTP asserts nothing, and whichever engine holds the port serves it.

Some subcommands are hidden and engine-internal. They appear here only so the
CLI surface is complete. Scripts and users do not invoke them.

## When to use this

- **Python scripts** (`apps/<name>/scripts/*.py`, `triggers/<name>/scripts/*.py`, `knowhow/*/scripts/*.py`): call `subprocess.run(['lucidos', 'data', 'write', ...])`. The CLI is on PATH and `LUCIDOS_WORKSPACE` is set. Runtime state the script rewrites on every run (a cursor, a last-seen id) is the exception: write it directly, as `plugins.md` § "Where a plugin keeps its runtime state" shows.
- **Bash scripts** (same locations, `*.sh`): call `lucidos` directly. PATH is set before the script runs.
- **Coding-agent subprocesses**: these run in a worktree under `<workspace>/.lucidos/worktrees/<id>/`. Editor writes land in the worktree, not the workspace, so dev-server links 404. Use `lucidos data write` instead.

App UIs in the browser use the JS SDK (`lucidos.data.*`, `lucidos.events.*`). The CLI is for shell and subprocess contexts only.

**A fact worth keeping goes in `data/`, never in the agent's own memory directory.** Claude Code writes per-user memory under `$CLAUDE_CONFIG_DIR/projects/<cwd>/memory/`, outside both the worktree and the workspace. The engine never indexes it, and Codex, the user and the next session on another machine never read it. Write the fact to `knowhow/<topic>/`, or the relevant app's or plugin's knowhow, with `lucidos data write`. In the Lucidos repo itself, a convention for coding agents goes in `CLAUDE.md` or `.claude/rules/`, committed with the change.

## Subcommands

### Hidden: `lucidos coding-agent-diff-hook`

A git `post-commit` hook in Lucidos-managed coding-agent worktrees. When the
process has `LUCIDOS_THREAD_ID` set, it posts the repo root and branch to the
parent engine, so `coding_agent_has_diff` refreshes right after a commit. It is
silent and best-effort. It creates no `ChangeProposed` event and no Apply-able
change: the formal proposal still happens when the coding-agent turn idles.

Do not call it from scripts. It needs the subprocess-origin headers the CLI
attaches in spawned subprocesses.

### Hidden: `lucidos cc-agent-guard`

The PreToolUse hook Claude Code runs before every `Agent` call in a Lucidos
session. It always refuses `isolation: "remote"`. The engine ends the Claude
Code process when the turn ends, and a background subagent dies with it, so its
report never arrives. Several foreground `Agent` calls in one message still run
in parallel. The engine sets `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` on every
session, so the schema has no `run_in_background` flag and every subagent runs
in the foreground. Only where that switch is off does the hook also refuse a
call that does not set `run_in_background: false`.

It also refuses a `subagent_type` whose agent definition sets
`background: true` or `isolation: remote` in its frontmatter. Claude Code runs
such an agent in the background whatever the call says, so the refusal asks
for another `subagent_type`. A call that sets its own `isolation` overrides
the definition's, and `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` overrides
`background: true`. The hook finds the definition where Claude Code does:

- `<dir>/.claude/agents/`, where `<dir>` is the session's `cwd` or a parent
  up to the git root, never home itself. The deeper directory wins.
- `$CLAUDE_CONFIG_DIR/agents/` (default `~/.claude/agents/`).
- `<add-dir>/.claude/agents/` for each directory the session was granted,
  such as the workspace's `data/`. These beat user agents and lose to project
  ones.
- `<managed dir>/.claude/agents/`, the managed-policy agents.
- Enabled plugins, for a `<plugin>:<name>` type.

The type is the frontmatter `name`, not the file name. Built-in types such as
`general-purpose`, `Explore` and `Plan` never set `background`. A directory
added mid-session with `/add-dir` is not covered. The hook lets a call
through when it cannot read a definition. Do not call it directly.

### `lucidos data path <relative> [--mkdir]`

Print the absolute filesystem path that `<relative>` resolves to inside the parent workspace's `data/` directory.

A path that starts with a `data/` tree the engine's data route accepts is kept as is. Those trees are `artifacts/`, `apps/`, `knowhow/`, `triggers/`, `config/`, `auth-modules/`, `scripts/`, `themes/` and `fonts/`. Anything else is prefixed with `artifacts/`. `normalizeDataPath()` in the UI follows the same rule, so `themes/harbour.json` means `data/themes/harbour.json` everywhere.

```bash
$ lucidos data path artifacts/data-analysis/foo/report.html
/Users/.../workspaces/myws/data/artifacts/data-analysis/foo/report.html

$ lucidos data path report.html
/Users/.../workspaces/myws/data/artifacts/report.html

$ lucidos data path knowhow/myapp/notes.md --mkdir
/Users/.../workspaces/myws/data/knowhow/myapp/notes.md
# (parent dir created)
```

`--mkdir` creates the parent directory chain, for piping the result to another tool.

### `lucidos data write <relative> [--from <local-path> | -]`

Write content into the parent workspace's `data/` tree. Creates parent dirs.

The write goes through the engine (`PUT /api/v1/data/*path`), not the
filesystem, so it needs a running engine, like `events emit` and `notify`. That
makes the file *arrive* in the workspace, not just appear on disk. The engine
commits it to the `data/` repo and announces it (`DataFileWritten`, plus
`ArtifactCreated` / `ArtifactUpdated` under `artifacts/`). So the Files panel
refreshes live, memory picks the file up under either *memory module*, an
`on_event: ArtifactCreated` trigger sees it, and the chat link below resolves.
Content is limited to 100 MiB per write. A failed write exits non-zero, prints
the engine's reason on stderr, and prints no link.

The engine checks a tree's rules before anything reaches disk. A file
under `themes/` must be a valid theme (`system-knowhow/themes.md` § "Make a
theme"). A file under `fonts/` must be a workspace font's `font.json` or one of
its font files (`system-knowhow/workspace-fonts.md`). A refused write exits
non-zero with the reason and writes nothing.

Under the Codex sandbox this works without a writable-root grant, because it is
an HTTP call rather than a write outside the worktree.

```bash
# from a local file you generated elsewhere
$ lucidos data write artifacts/data-analysis/2026-04-20/report.html --from /tmp/report.html

# from stdin (default; also `--from -`)
$ echo '{"hello": "world"}' | lucidos data write artifacts/foo.json
```

Two outputs:

- **stderr**: the resolved absolute filesystem path, so you can capture it separately (`… 2>/tmp/path`). The path is always the last line.
- **stdout**: a ready-to-paste clickable Lucidos chat link, like `lucidos spawn-thread` prints:

```bash
$ echo '# notes' | lucidos data write artifacts/ticket-workflow/node-types-and-attributes.md
[node-types-and-attributes.md](artifacts/ticket-workflow/node-types-and-attributes.md)   # stdout
The user cannot see this file yet. ...                                                   # stderr
/Users/.../workspaces/myws/data/artifacts/ticket-workflow/node-types-and-attributes.md   # stderr
```

Saving an artifact shows the user nothing, and neither does a link they never
receive. Before a question card, put the link line in the card's question: an
option shows a link as plain text. Your words before any tool call reach the
user only as a short summary, which drops the link. Otherwise paste it in the
reply that ends your turn. A file outside `artifacts/` gets no reminder, unless
it is a picture.

A picture (`gif`, `jpeg`, `jpg`, `png`, `svg`, `webp`) prints as a markdown
**image**, which shows inline in the thread. A plain link to a picture only
opens a preview on tap. So paste the line as printed:

```bash
$ lucidos data write artifacts/design/options.png --from /tmp/options.png
![options.png](artifacts/design/options.png#1600x1200)   # stdout
The user cannot see this picture yet. ...                # stderr, before the path
```

The `#1600x1200` ending is an *image size hint*: the picture's size in pixels.
The thread holds the picture's space before it loads, so nothing jumps. Keep it
when you paste the line. A picture whose size the CLI cannot be sure of (an
SVG, or a photo with EXIF data) prints without one.

Saving a picture shows the user nothing. It appears only where you paste that
line. If a question card comes next, put the line on the card: in its question,
or in an option's `preview`. For a choice, give each option its own picture of
only that option, never one sheet of them all in the question. Your words
before any tool call reach the user only as a short summary, which drops the
picture. Otherwise paste it in the reply that ends your turn.

**Linking an artifact in chat: use the bare store path, never a scheme.** The clickable form is the `data/`-rooted path with no URL scheme (e.g. `artifacts/ticket-workflow/node-types-and-attributes.md`, or with the leading `data/`), which the frontend's path linkifier turns into a file-preview link. There is **no `artifact:` or `file:` scheme**, and one invented by analogy to `thread:`/`app:` is a dead link. Paste the stdout link verbatim, or keep its target and swap in a friendlier label: `[OST node types & attributes](artifacts/ticket-workflow/node-types-and-attributes.md)`.

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
- `payload` must be a JSON **object** with a `summary` string. `--summary` sets or overrides `summary` in the payload before sending.

```bash
$ lucidos events emit AnalysisCompleted \
    --summary "Data analysis for 2026-04-20 complete" \
    --payload '{"artifact": "artifacts/data-analysis/2026-04-20/report.html", "rows": 1240}'
```

The CLI prints the server's JSON response on stdout (`{"success": true, "event_id": "..."}`).

### `lucidos events query [--type T] [--since iso] [--until iso] [--thread-id UUID] [--before-event-id UUID | --after-event-id UUID] [--limit N]`

GET events from the parent workspace's event store. Outputs the raw JSON array on stdout, newest-first.

This reads the **whole** store, not only what the workspace emitted. Engine thread and system events (`ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`) are rows in the same table. See `system-knowhow/thread-events.md` § "One table, two enums".

`--thread-id` narrows to one thread, which is how you READ a past conversation. Pair it with `--type MessageReceived` (or `ResponseGenerated`), or the query returns that thread's whole transcript, every streamed token included. Without it the query covers every thread.

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

Paging backwards with `--until` breaks at the page boundary: events sharing one timestamp get duplicated or dropped. The cursor flags order results on `(created, id)`, which stays stable even when many events share a millisecond.

- `--before-event-id <UUID>`: only events strictly older than that event. For backwards paging.
- `--after-event-id <UUID>`: only events strictly newer than that event. For tail-following.

Passing both is a `400 Bad Request`. A cursor that does not exist is a `404 Not Found`, not a silently empty result.

```bash
# Page 1: newest 100.
PAGE=$(lucidos events query --type BrowserLearningObserved --limit 100)
OLDEST_ID=$(echo "$PAGE" | jq -r '.[-1].id')

# Page 2+: strictly older than the last event of the previous page.
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
- **Without `--type`:** `{"by_type": [{"event_type": "...", "count": N, "byte_total": B}, ...], "total_count": N, "total_byte_total": B}`, a per-type breakdown sorted by `count` desc.

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

`byte_total` is `SUM(octet_length(payload::text))`, the raw payload byte sum. It is a reliable proxy for the token cost of the matching `lucidos events query`. On a busy workspace, run it before `query` to pick which types to drill into. A `query --type ToolResult --limit 300` can return 2.3 MB and blow the next turn's prompt cap.

### `lucidos threads list [--active | --status <list>] [--source <list>] [--limit N] [--parent <uuid> | --my-children] [--has-draft [false]] [--has-diff [false]]`

List thread summaries from the parent workspace. Outputs the raw JSON array on stdout, newest-first by `last_activity`. Each row is a full `ThreadSummary`, the shape `lucidos.threads.list()` returns in the JS SDK, plus the *reader fields*:

- `has_draft`: whether the thread holds an unsent *draft*.
- `draft_preview` and `draft_length`: its first 200 characters and its length in characters, present only when it does.
- `link`: the *thread link*, `thread:<workspace>/<thread_id>`. Paste it as a markdown link target to point the user at the thread.

The `threads` LLM tool's `list` returns the same rows without the raw `compose_*` fields. It reads a whole draft through `drafts` instead.

```bash
$ lucidos threads list --status running --limit 5 | jq '.[].title'
"Plan dinner"
"Refactor settings dialog"
```

#### Picking between `--status` and `--active`

**`--status running` is what "is the workspace busy?" means.** `--active` is the **union** of `running` and `waiting_for_user_answer`, and those two are opposites. `running` is the workspace working. `waiting_for_user_answer` is the workspace stopped, waiting on a person, and most likely to hide work piling up. So an idle detector gated on `--active` never fires while anybody is being asked something.

- `--status <list>` restricts to exactly the statuses you name, out of `idle`, `running`, `waiting`, `waiting_for_user_answer`, `paused`, `failed`. These are the values each row's `status` field carries, so you can filter on what you read. The kebab spelling `waiting-for-user-answer` works too. Repeatable (`--status running --status failed`) and comma-separated (`--status running,failed`) are the same request. An unknown or empty value is an error listing the valid ones, never a silently empty list.
- `--active` selects the union above. Use it only for "the loop is mid-flow in either direction", such as a badge counting threads the user has something invested in. Passing it with `--status` is refused: they are two answers to one question.
- The `--active` union never contains `failed` (the response is over, errored or interrupted, with nobody resuming it). Nor `paused` (the user's own version switch interrupted the turn, and the engine resumes it by itself). Only `--status` reaches them, by name. It also reaches `waiting`, which nothing writes any more: it meant the coding agent stopped with changes to review, and only older rows carry it.
- `--source` is a comma-separated list of `chat`, `trigger`, `coding-agent`. Legacy `claude_code` is also accepted. Omit for all sources.
- `--limit` clamps to `1..=1000` server-side, default 100.
- `--parent <uuid>` restricts to that thread's **direct** children, never its grandchildren. A malformed uuid is a 400, never a silently unfiltered list.
- `--my-children` is `--parent` with the calling thread's own id, read from `$LUCIDOS_THREAD_ID`. Use it to recover a child's `thread_id`, see which children still work, and spot one parked on a question. Outside a Lucidos-spawned subprocess it errors, rather than listing the whole workspace. Passing both child filters is refused: one filter, one answer.
- `--has-draft` keeps threads holding an unsent draft, and `--has-draft false` keeps the rest. `--has-diff` does the same for a coding-agent branch that differs from main. A pending question and a failure are statuses: filter them with `--status waiting_for_user_answer` and `--status failed`.

```bash
# Which of my own children are still working, and what are they called?
$ lucidos threads list --my-children --status running | jq -r '.[] | "\(.status)\t\(.title)\t\(.thread_id)"'

# Which of them are stuck waiting on me?
$ lucidos threads list --my-children --status waiting_for_user_answer | jq -r '.[].title'

# Which of them have changes waiting below them?
$ lucidos threads list --my-children | jq -r '.[] | select(.pending_sub_thread_change_count > 0) | .title'
```

Every row carries `pending_sub_thread_change_count`: the pending changes held by that thread's sub-threads at any depth, not its own. An orchestrating child that ran children of its own holds no change itself, so this is where the work below it shows. `lucidos changes list --sub-threads-of <uuid>` lists those changes.

Use this when a script reacts to thread state ("is anything still running before I fire this trigger?"). It reads the projection's per-thread status, so never rebuild that from raw `query_events`.

### `lucidos threads count [--active | --status <list>] [--source <list>] [--parent <uuid> | --my-children] [--has-draft [false]] [--has-diff [false]]`

Count thread summaries matching the same filters as `list`, including `--status`, the two child filters, `--has-draft` and `--has-diff`. Outputs `{"count": N}` on stdout.

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

On a big workspace this is cheaper than reading `.length` off the full list.

### `lucidos threads drafts [--thread <uuid>] [--limit N]`

List every thread holding an unsent *draft*, newest edit first, as a JSON array. Wraps `GET /api/v1/threads/drafts`, the same read as the `threads` tool's `drafts` action. Read-only: nothing on any agent surface writes a draft.

Each row carries `thread_id`, `title`, `channel`, `state`, `section`, `status`, `parent_thread_id`, `preview`, `length`, `image_count`, `last_edited` and `link`. `state` is `composing` for a thread never sent and `active` for one with history. `preview` is the first 200 characters, and `length` counts characters. `last_edited` is `null` for a draft typed before the engine recorded edit times.

`--thread <uuid>` prints that one draft as an object, with its whole `text`. A thread holding no draft is a 404. `--limit` clamps to `1..=1000`, default 100.

**A draft has no link of its own.** Its `link` opens the thread that holds it, which is where the user finds the composer with the text in it.

```bash
# Which threads hold a draft, and where?
$ lucidos threads drafts | jq -r '.[] | "[\(.title)](\(.link))\t\(.preview)"'

# Read one draft whole.
$ lucidos threads drafts --thread 9c1f2b40-... | jq -r .text
```

### `lucidos threads held-messages [--limit N]`

List every *held message* still waiting, newest first, as a JSON array. An agent-sent message to a coding-agent thread is held while that thread waits on the user, and released when they answer. Wraps `GET /api/v1/threads/held-messages`, the same read as the `threads` tool's `held_messages` action. Read-only.

Each row carries `held_message_id`, `thread_id`, `title`, `channel`, `section`, `status`, `preview`, `length`, `image_count`, `held_at` and the owning thread's `link`. `--limit` clamps to `1..=1000`, default 100.

### `lucidos threads search <query> [--limit N]`

Find threads by what was said in them or typed into their draft. Wraps `GET /api/v1/threads/search`: title, message content and draft text, plus semantic matches. A draft matches only when it holds every word of the query. Outputs `{"results": [...]}`, each a full `ThreadSummary` with the reader fields and a `score`. `--limit` clamps to `1..=50`, default 20. Read-only.

```bash
$ lucidos threads search "cabin keys" | jq -r '.results[] | "[\(.title)](\(.link))"'
```

### `lucidos threads follow-up --thread <child-uuid> --message <M> [--event-id <E>] [--urgent]`

Send a message to one of **this thread's own child threads**, or from the *home thread* to any thread: redirect one going the wrong way, hand it something a sibling learned, or tell a stalled one to continue. This is the *child follow-up* edge, the one privileged cross-thread write. Wraps `POST /api/v1/threads/<child>/follow-up`.

```bash
# Redirect the child that is taking the wrong approach.
$ lucidos threads follow-up \
    --thread 9c1f2b40-... \
    --message "Skip the CSV path entirely, the source is a live API."
{"child_thread_id":"9c1f2b40-...","reach":"own-child",
 "child_title":"Import the sales figures","delivered_to":"running",
 "detail":"The child was mid-turn, so this queues behind its current work or steers it."}
```

The ack prints as raw JSON on stdout, like every other `lucidos threads` subcommand. The chat agent's equivalent is the `follow_up_child_thread` tool, with the same reach.

- **You can only address your own DIRECT children.** No siblings, no grandchildren, no arbitrary thread. No flag says who you are: the engine reads the calling thread off the *thread-bound origin token* this subprocess was spawned with, then checks the child's own row. A thread that is not yours is a 403 whatever you claim.
- **The *home thread* is the one exception.** It may address any thread in its workspace, coding-agent threads included, and the ack's `reach` reads `home`. A thread it did not spawn reports to its own parent, not to Home, so read its reply with `lucidos events` afterwards. Every other caller's ack reads `reach: own-child`.
- **This is a boundary, not an obstacle.** Outside the home thread, if asked to message a thread you did not spawn, there is no other route. Say so, name the threads, and let the user send it (see the top of this file).
- **It returns as soon as the message lands, and does not wait for the child.** The child reports back the usual way, as a completion card on its parent. The ack's `delivered_to` says which of four things happened:
  - `running`: the child was mid-turn, so the message queues behind its work or steers it.
  - `interrupted`: you passed `--urgent`, and the child's turn is stopping so it reads you next.
  - `waiting-for-user-answer`: parked on a question or permission card, so **a human must answer before it reads this**.
  - `revived`: it was not working, so a fresh turn starts now.

  `detail` says the same in a sentence.
- **`--urgent` is for cancellations, not for hurry.** By default a mid-turn child reads you at its next natural break. Inside a long tool call that can take many minutes: a child in a ten-minute blocking wait reads you when the wait returns. `--urgent` stops the child's current turn so it reads you at once, and that turn's unfinished work is lost. Use it when the child must act on your message *instead* of what it is doing.
- **Address the child by uuid, never by title.** Titles are not unique, and a fuzzy match would deliver to the wrong child. Find the id with `lucidos threads list --my-children`. Afterwards, refer to the child by the ack's `child_title`: a uuid names nothing the user can see.
- **A follow-up is never refused at the child cap.** The cap limits spawns, not messages, so reviving a child is cheaper than spawning another. The revived child counts as live again while it runs.

`--event-id` defaults from `$LUCIDOS_EVENT_ID` and stamps the child's message-route panel, so the follow-up links back to the originating event.

**A cancellation is not done when the ack returns.** The ack says only that the message is on the child's timeline. Even with `--urgent`, the child still has to read it and do the work of stopping. If you told a child to kill a job, verify the job is gone (no processes, no lock file) before reporting the cancellation complete.

### `lucidos threads detach --thread <child-uuid>`

Move a child thread to **top level**, so its parent stops waiting for it. Wraps `POST /api/v1/threads/<child>/detach`, the same move the thread menu's **Move to top level** makes.

```bash
$ lucidos threads detach --thread 9c1f2b40-...
{"child_thread_id":"9c1f2b40-...","child_title":"Import the sales figures",
 "former_parent_thread_id":"4b7e0c11-..."}
```

- **From inside a thread you can move only your own DIRECT children.** The engine reads the calling thread off the origin token, exactly as for `follow-up`. Anything else is a 403. Outside a thread, with no origin token, it moves any thread that has a parent.
- **Nothing is stopped.** The child keeps running, finishes on its own and proposes any change. Its result goes to its own timeline only.
- **The former parent gets nothing more.** No completion card, no follow-up, and it leaves `--my-children`. A card the child earned before the move still arrives.
- **It frees no child slot while the child runs.** The moved child counts against the former parent's cap until it finishes.
- **It cannot be undone.**
- A thread already at top level is a 409, and an unknown id a 404.

### `lucidos threads archive --thread <current|child-uuid>`

Archive this thread, or one of its own direct children (ADR 0310). Wraps `POST /api/v1/threads/<id>/archive`, which runs the same cascade as the Archive button.

```bash
$ lucidos threads archive --thread 9c1f2b40-...
{"archived":["9c1f2b40-..."],"skipped":[]}

$ lucidos threads archive --thread current
{"requested":"4b7e0c11-...","detail":"This thread is archived once its turn ends and it has settled."}
```

- **From inside a thread you can archive only yourself and your own DIRECT children.** The engine reads the calling thread off the origin token, and resolves `current` to it. Anything else is a 403 with `not_your_thread`, an unknown id a 404, and a discarded thread a 409. Outside a thread, with no origin token, it archives any thread, as the Archive button does.
- **A child is archived now**, with its own sub-threads. **`current` is archived once this turn ends** and the thread has settled. A new message into it before then keeps it open.
- **The home thread is never archived**, by anyone: a 409 with `home_thread`.
- **The Archive button's refusals apply.** A running target or one waiting on the user is a 409 with `parent_not_archivable`. A pending change is `parent_has_pending_changes`, and a blocking sub-thread is `descendants_blocking`. Each body carries a `message` saying what to do. A pinned target is a 409 with `thread_pinned`: only the user archives a pinned thread. A pinned sub-thread of the target stays open and is listed under `skipped`.
- **Archiving lets the worktree be reclaimed**, so a later follow-up rebuilds it.

### `lucidos spawn-thread --to <WS> --message <M> [--image <path> ...] [--coding-agent <backend>] [--folder <path> | --repo <name>] [--relation child|top] [--title <T>] [--model <M>] [--coding-agent-model <M>] [--reasoning-effort <level>]`

Start a new *thread* in another (or this same) workspace: a *chat thread* by default, or a *coding-agent thread* with a coding-agent flag. `--to` takes an absolute path, or a bare workspace name. A bare name resolves against `$LUCIDOS_WORKSPACES_ROOT` when set, else the directory holding your own workspace, else `~/workspaces`. So a sibling workspace is always reachable by name. Caller provenance (`caller_*` fields) defaults from `$LUCIDOS_WORKSPACE` / `$LUCIDOS_THREAD_ID` / `$LUCIDOS_EVENT_ID`, which the engine sets on every spawned subprocess. Prints a clickable `[title](thread:<ws>/<uuid>)` markdown link on stdout.

A workspace on another install is not a sibling: a packaged app and a dev checkout serve separate workspace sets, so pass its absolute path. That is the only route there, since `run_coding_agent(workspace=…)` takes a basename alone.

`--relation top` (the default) starts an independent thread that does not report back. `--relation child` is a same-workspace spawn with a callback: the calling thread auto-resumes when the child finishes.

**A spawn needs something to say.** A blank `--message` with no `--image` exits non-zero before any request, and the target engine refuses one with a 400 too. No thread is created either way. The usual cause is `--message "$(cat brief.md)"` where `brief.md` was never written, so guard it: `test -s brief.md && lucidos spawn-thread … --message "$(cat brief.md)"`.

A top-thread sits under the workspace, not under you, so creating one is the *workspace owner*'s call (ADR 0168). From a thread, `--relation top` needs their standing instruction: a turn they opened, or a trigger firing they authorized. Without one it exits non-zero on a 403 whose body names those two shapes. `--relation child` stays inside your own subtree and needs nothing.

**Images:** `--image <path>` attaches an image file to the message. Repeat it for several. The target engine judges each one exactly as it judges an image attached in the app. It reads the format from the bytes, takes PNG, JPEG, WebP, GIF and HEIC, and fits a large image to the model's size limit. Any other format fails the whole spawn with a 415 naming it, before any thread starts. The file extension does not matter, and a missing file fails before any request.

The images travel inside the spawn request as base64, which the engine caps at 100 MiB. So keep one spawn's images under about 75 MiB in total. A larger set fails with a 413.

**Coding-agent backend:**

- `--coding-agent <backend>`: the backend selector, and the spelling to use. Valid values are `claude-code` (alias `claude_code`) and `codex`. Passing it implies coding-agent mode.
- `--cc` and `--codex` are legacy shorthands for the two backends. They still work, but write `--coding-agent`.

**Model and reasoning level (either backend):**

- `--coding-agent-model <m>`: the model the coding agent runs on (`sonnet`, `opus`, `gpt-5.6-sol`).
- `--reasoning-effort <level>`: how hard the coding agent thinks. One of `low`, `medium`, `high`, `xhigh`, `max`. It pins the level for this spawn only, over the backend's own default. It needs a coding-agent flag, and the CLI refuses an unknown level before sending anything. A chat-thread spawn reads a different ladder and does not take this flag.

**Codex offers `max` on the GPT-5.6 models only, so pair the two.** `--reasoning-effort max --coding-agent codex` needs `--coding-agent-model` naming one of those models, or the CLI refuses the spawn. Naming no model counts too: the engine tests against the model in the request, so `max` with no model is dropped before Codex is asked. Every level below `max` is unrestricted on both backends and needs no model.

**Worktree targeting for coding-agent threads:**

- `--repo <name|uuid>`: create the worktree from a registered *repository*. Defaults from `$LUCIDOS_REPO` (the calling thread's repo), so a coding-agent sidequest stays in its caller's repo. Pass `--repo ""` to force the target workspace's default repo.
- `--folder <path>`: target an app folder instead, spawning an **app coding-agent thread**. A `data/apps/<id>` value (workspace-relative, resolved on the *target* workspace) creates a sparse-checkout worktree narrowed to that app folder. Its *Apply* ff-merges into the workspace's `main`, with no `/harden` and no engine restart. The `run_coding_agent` tool's `folder` argument uses the same machinery. Only whole app folders are valid: the engine rejects other `data/` subtrees, app subpaths, and folders that do not exist.

`--folder` and `--repo` are mutually exclusive. `--folder` requires a coding-agent flag (`--coding-agent`, `--codex`, or `--cc`), or the CLI errors before any HTTP round-trip. `--folder` suppresses the `$LUCIDOS_REPO` default, since the engine rejects a request carrying both a repo and a folder.

```bash
# Spawn an app coding-agent thread to work on an app in this workspace.
$ lucidos spawn-thread --to myws --coding-agent claude-code \
    --folder data/apps/habit-tracker \
    --title "Research session" \
    --message "Run one research session per app knowhow."
[Research session](thread:myws/2f1c…)

# Spawn a Codex coding-agent thread in the dev workspace.
$ lucidos spawn-thread --to dev --coding-agent codex \
    --title "Codex review" \
    --message "Review the app folder and fix the failing test."
[Codex review](thread:dev/7a42…)

# Ask for the hardest thinking the backend offers, for this spawn only.
$ lucidos spawn-thread --to dev --coding-agent claude-code \
    --reasoning-effort max \
    --title "Ideation session" \
    --message "Explore three designs for the capacity policy."
[Ideation session](thread:dev/9b07…)
```

### `lucidos await-event --on <EventType> [--on <EventType> ...] [--condition <JSON>] --timeout-secs <N> --reason <R>`

Subscribe the **calling thread** to a Lucidos event, then finish. The engine
re-opens the thread with a follow-up message when a matching event lands, or
tells it the deadline passed. It is the coding-agent counterpart of the chat
agent's `await_event` tool, on the same registration, so both get the same caps
and refusals.

**It returns immediately and blocks nothing.** The thread is plain **idle** while
it holds a subscription: no queue slot, no running turn, nothing for the user to
resolve. So the shape is *subscribe, say what you are waiting for, end the
session*. It replaces a sleep-and-recheck loop. Polling for the event as well is
worse than either.

Use it whenever you wait on something the engine emits: a change appearing
(`ChangeProposed`), a trigger firing (`TriggerExecuted`), a backup finishing
(`BackupCompleted` / `BackupFailed`), or a domain event your own scripts emit.
Any persisted event works. A transient frame such as `BackupProgress` is refused
by name. It is **not** for external state with no Lucidos event (a third-party
API, a file another process may write): nothing would be delivered, so poll.

**And never subscribe to your own child's completion.** A thread spawned with
`lucidos spawn-thread --relation child` already re-opens this one. When it
finishes, the engine emits `ChildThreadCompleted` here and re-opens this thread
with the child's status, summary, `pending_change_ids` and
`sub_thread_pending_changes`. A wait on it buys nothing and costs two things:
one of the subscriptions the loop cap allows, and a second clock. A child that
outlives `--timeout-secs` re-opens this thread with a pointless expiry, then
again when it finishes.

Await a `ChildThreadCompleted` only for a completion that is **not** your own
child's, named with `--condition '{"child_thread_id": "<uuid>"}'`. Matching is
workspace-wide, so that is any thread's child, not only a descendant of yours.
A coding-agent session another thread spawned is a first-class thing to watch.
A session **nobody** spawned (the user started it) has no
`ChildThreadCompleted`, since only the parent/child fan-in emits one. Watch its
turn boundary instead: `--on CodingAgentIdled --condition '{"thread_id": "<uuid>"}'`.
That fires on every idle, a user Stop included, so it means "the agent went
idle", not "the work is done".

A **rendezvous, not a stream**. The first match resolves the subscription and
consumes it. "Continue when the next X happens" is this. "React to every X,
forever" is a *trigger*.

**It watches forward only, so still check whether it already happened.** A
subscription cannot fire for an event already gone by, so look at state first.
The race between that check and this command is covered: if a match landed in
the few minutes before it, the response names it, with its age. Read that part,
not just the `"status":"subscribed"`, and act on it before you finish, because
nothing will deliver it. Only you can tell an event you missed from one you
handled a few minutes ago.

- `--on` names the event type, PascalCase past tense. Repeat it to watch
  several: any one of them re-opens the thread.
- `--condition` is a JSON object filtering the event's OWN payload by field
  path (a dot reads one level down), applied to every `--on` name. Equality by
  default, or an operator object: `{"$eq":v}`, `{"$ne":v}`, `{"$lt":n}`,
  `{"$lte":n}`, `{"$gt":n}`, `{"$gte":n}`, `{"$in":[…]}`, `{"$nin":[…]}`,
  `{"$regex":"…"}`. `$or` in key position takes a list of whole conditions.
  See `system-knowhow/triggers.md` § "What a condition can say" for the full
  language.
- A thread event always offers one field beyond the payload: `thread_id`, from
  the thread the event belongs to. So `--condition '{"thread_id":"<uuid>"}'`
  scopes the wait to one thread. It does not appear in `lucidos events query`
  output (the row holds the thread in its own column). A **domain event**
  belongs to no thread, so it has none.
- `--timeout-secs` is required and capped at 86400 (24 h). There is no unbounded
  subscription. Giving up early costs one turn. Giving up too late costs the
  user the whole wait.
- `--reason` is a short noun phrase in the user's language, naming **what** you
  await. They read it in the waiting indicator, and it tells a sleeping thread
  from a stalled one. Write `"the e2e lock to free up"`, not
  `"waiting for the e2e lock"`: the transcript labels it `Waiting for <reason>`,
  so a waiting word says it twice.

Refusals arrive as a `400` with the reason. Read it rather than retrying:

- A per-token streaming event (`TextStreamed` and friends) or an `EventWait*`
  type is refused outright.
- A thread may hold at most 25 live subscriptions.
- The same `--on` list twice on one thread is refused (it would deliver one
  event to you twice).
- The loop cap: 20 counted subscriptions within an hour with no message or
  answer from the user. A subscription another thread's event ended does not
  count.

```bash
# Wait for a domain event the workspace's own scripts emit, then stop. The
# engine re-opens this thread with the payload when it lands.
$ lucidos await-event --on E2ETestsPassed --timeout-secs 3600 \
    --reason "tonight's e2e run to report"

# Narrow it: only a change that actually touched files.
$ lucidos await-event --on ChangeProposed --condition '{"file_count": {"$gt": 0}}' \
    --timeout-secs 1800 --reason "the refactor to propose its change"
```

### `lucidos background-task run [--description <T>] [--timeout-secs <N>] -- <command>` / `output <task_id>` / `stop <task_id>`

Run work that has to outlive your turn, then **end your turn**. The engine runs
the command as a *background task* in this thread's worktree. It arms an event
wait on the task's `BackgroundBashCompleted` before `run` returns. When the task
finishes, the engine re-opens this thread with the exit status and the tail of
its output. Nothing waits in the meantime, so no turn re-reads its context.

Use it for anything longer than a foreground call can hold: a full test suite,
an e2e run, a release build. A command you background yourself (`&`, `nohup`)
dies when your turn ends, because the engine stops your whole process group.
Claude Code's own background mode is off in a Lucidos session, so a foreground
call that outruns its timeout is killed. Anything that surely fits in 10 minutes is
simpler as one foreground call with the maximum timeout.

- **One argument after `--` runs as written**, so quote a pipeline or a
  redirect as a single string. Several arguments run as exactly those words.
- **Always pass `--description`**: what the task is, as a short noun phrase
  in the user's language (`"the nightly e2e sweep"`). The thread's waiting row
  reads `Waiting for <description> to finish`, which tells the user what the
  thread waits on. Without it the row shows the command line.
- `--timeout-secs` kills a task still running after that long. Default and
  maximum: 3600.
- `run` prints a `status`. `watched` means end your turn now. `unwatched` means
  the thread hit a subscription limit and nothing will wake it: stop the task
  and run the command in the foreground. `finished` means it was quick, and the
  output is already in the response.
- The re-open message carries each output stream's last 4000 bytes. `output
  <task_id>` prints more: what arrived since your last read while the task
  runs, and its final record once it has finished.
- Only this thread's own agent can start, read or stop its tasks. The engine
  refuses a command on the catastrophic deny-list.
- The task gets the same environment as your own shell, so no `CRED_*` or
  `OAUTH_*` secrets. Reach an external API through `lucidos proxy`.
- Discarding or archiving the thread kills its running tasks. Stopping a task
  records its completion as killed, and that still re-opens the thread.
- **The exit status is the command's own.** Do not end a command with
  `; echo $? > file`: the `echo` succeeds, so the task reports exit 0 even when
  the work failed. To keep a copy in a file as well, pass the status on:
  `…; rc=$?; echo $rc > .lucidos/run.exit; exit $rc`.

**Stopping your own task does not re-open your thread.** `stop` stands down
this thread's waits on the task before it signals it, so the killed completion
wakes nobody here (ADR 0369). If one of your own waits watched more than this
task, `stop` ends it whole and names it, so you can re-arm the rest.

**A stop, a timeout or a Discard ends the task's whole process group.** The
engine sends SIGTERM, waits 3 s so a trap can clean up, then sends SIGKILL. A
process that detached into its own session is out of reach. A task that exits
on its own signals nothing, so a `nohup … &` it started keeps running. The chat
agent's `run_bash_background` behaves the same way.

```bash
# The normal shape: start the suite, then end your turn.
$ lucidos background-task run --description "the engine test suite" \
    -- 'lucidos build-slot -- cargo test > .lucidos/test.log 2>&1'

# Look in on it, or give up on it.
$ lucidos background-task output 5f0c2e1a-…
$ lucidos background-task stop 5f0c2e1a-…
```

### `lucidos build-slot [--label <T>] [--max-wait <SECS>] -- <command>` / `--status` / `--set-capacity <N>`

Run a heavy build under a *build slot*, so parallel *worktrees* cannot pile N
full compiles onto one host, OOM it, or bury its cores.

**Wrap anything heavy** that a coding-agent session runs: `cargo build`,
`cargo test`, a Gradle or Xcode build, a large bundler run. The slot is taken
before the command starts and freed when it exits, or when this process dies.
Do NOT wrap cheap work (a type-check, a unit-test run of a small package):
it would sit in a slot for minutes to save seconds.

**A granted slot also shapes the build.** It runs at `nice +10`, which the
whole compile tree inherits, and gets a share of the cores as
`CARGO_BUILD_JOBS`. The share is the host's cores divided by the slots held
right now. It never falls below the fixed `cores / capacity` share, and never
below 1. So a solo build keeps the machine, and contention divides it.

```bash
# The normal shape. Blocks until a slot frees, then runs the build.
$ lucidos build-slot -- cargo test --release
lucidos build-slot: slot 0 of 3, nice +10, 18 cores

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
build scripts already take a slot, so type the ordinary command. Use the
wrapper in any OTHER repo.

**It waits, it does not fail.** The second build is wanted, just not at the
same time, so it blocks with no deadline and prints progress.
`--max-wait <secs>` opts into a deadline and exits **75** when it passes. If
you set one and hit it, do not retry on a timer: subscribe and end your turn.

```bash
$ lucidos await-event --on BuildSlotReleased --timeout-secs 3600 \
    --reason "a build slot before running the test suite"
```

- **Nesting is safe.** A wrapped command that wraps again runs straight
  through, so a script you call cannot deadlock against the slot you hold. It
  changes nothing either: the outer slot's priority and core share stand, and
  are not applied a second time.
- **A `CARGO_BUILD_JOBS` you set yourself always wins**, and nothing is
  exported over it.
- **`LUCIDOS_BUILD_SLOT_NICE` overrides the increment**, and `0` opts out
  entirely, which suits a foreground build you are waiting on. A non-root
  process cannot lower a nice increment again, so choose before the build
  starts.
- **The Apply build goes first.** The engine's own background rebuild waits
  as a *priority waiter*: while it waits, your build leaves a freed slot to
  it, and `--status` says so. It never takes a slot beyond the count, and it
  never stops a running build. Do not set `LUCIDOS_BUILD_SLOT_PRIORITY`
  yourself: it exists for the build the user is watching.
- **It never blocks a build it cannot govern.** With no `lucidos` binary, no
  writable pool, or no engine to announce to, the command just runs. A host
  that will not report its core count gets no share, and the build runs at
  cargo's own default.
- **The exit code is the command's**, and a signalled command reports
  `128 + signal`, so a killed build never reads as a pass.
- **Every release is announced.** `BuildSlotReleased` fires whenever a slot
  frees, so a session that gave up on `--max-wait` and subscribed is always
  woken. `BuildSlotWaiting` and `BuildSlotAcquired` fire only under contention.

### `lucidos event-waits list` / `lucidos event-waits cancel [--wait-id <ID>] [--on <EVENT_TYPE>] [--all]`

Read and stop the **calling thread's** own subscriptions, the ones
`lucidos await-event` armed. They are the coding-agent counterparts of the chat
agent's `list_event_waits` and `cancel_event_wait` tools, on the same code, so
both get the same report and refusals. Like `await-event`, both take the thread
from `$LUCIDOS_THREAD_ID` and have no thread flag, so neither can reach another
thread's subscriptions.

**`list` is the only way to answer "am I still watching for that?"** Nothing
tells you when a subscription ends. A delivery re-opens the thread, but a
timeout or a user pressing **Stop waiting** lands while your session is not
running. A subscription is *spent* the moment it fires. Answering from memory
is a guess. Run `list` before saying you are still watching, before
re-subscribing (a duplicate is refused), and to get the id `cancel` takes.

Each entry carries the subscription's id, the events and conditions it watches,
its `--reason`, when it was armed, and when it times out, with both ages
spelled out:

```bash
$ lucidos event-waits list
{"count":1,"event_waits":[{"wait_id":"3f2b…","subscription":"ChangeProposed",
  "reason":"the refactor to propose its change",
  "armed_at":"2026-08-07T09:14:22Z","armed_ago":"7m",
  "expires_at":"2026-08-07T09:44:22Z","expires_in":"22m"}]}
```

**`cancel` is how you stop watching.** A live subscription re-opens this thread
later whatever you told the user. So when they say stop, drop it, or never
mind, run this rather than promising. Use it too when the thing already
happened, or when a new subscription supersedes an old one.

Pass exactly one of `--wait-id <ID>` (from `list`), `--on <EVENT_TYPE>`, or
`--all`. None is the default: a bare call would have to guess between one and
all, and both guesses are wrong. Stopping is silent: the subscription ends, the
user sees it leave the waiting indicator, and the transcript records the stop.

**Reach for `--on` when the answer arrived some other way.** It is the safe
middle: it needs no id from `list`, and it leaves every other watch on this
thread standing, which `--all` does not. It ends every subscription watching
that event type, whatever `--condition` each one carries.

One sharp edge: a subscription can watch several event types (repeated `--on`
at `await-event`). Naming ONE of them ends that whole subscription, the other
names included. A wait is a single rendezvous, spent by the first match, so no
leg is left once you stop watching one. The result names every type it ended,
so read it. To keep watching the rest, arm a new subscription for them.

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

Refusals arrive as a `400` with the reason:

- more than one flag, or none;
- a `--wait-id` not live on this thread (fired, timed out, stopped, or another
  thread's: all look the same and mean "not yours to stop");
- an `--on` nothing on this thread watches. Read that one: a watch you thought
  was armed is not.

Scripts use this too. In `scripts/lib/e2e_lock.sh`, a run that takes the
machine-wide e2e lock runs `lucidos event-waits cancel --on E2ELockReleased`,
since holding the lock answers any watch for its release. The call is best
effort and its refusal is discarded, since most runs never subscribed.

### `lucidos notify --title <T> --message <M> [--app-id <APP>] [--tap <T>] [--thread-id <UUID>] [--event-id <UUID>] [--fragment <FRAG>]`

Send a push notification via the parent workspace. It persists to the inbox AND fans out as a web push to subscribed devices, exactly like a `send_notification` LLM tool call. Any subprocess (Python script, bash script, scheduled `script:`-typed trigger) can call it without an LLM thread.

```bash
$ lucidos notify --title "Nightly backup done" --message "Backup completed: 1,240 rows archived"
{"success":true,"notification_id":"5b1e..."}
```

`--title` and `--message` are both required and non-empty (the engine returns 400 on an empty value).

`--app-id <id>` is optional and stamps the notification's deep-link target. **Set it only when tapping should open that app to act on it.** Most reminders, nudges and summaries do not deep-link, even when their trigger lives inside an app dir. The `send_notification` LLM tool follows the same rule.

#### Deep-linking back to the originating event

For event-driven triggers ("coding agent is asking", "credential needed", …) the tap should land on the exact card the user must act on. Four flags wire this up:

- **`--tap <modal|navigate>`**: which kind of tap. `modal` (default) opens the inbox detail. Use it for purely informational pushes too ("Backup complete", "Sync finished"): every notification is openable, and there is no passive kind (the old `none` kind is retired). `navigate` deep-links to the target the other flags name: `--thread-id` opens that thread (scrolling to and pulsing `--event-id` when set), and `--app-id` opens that app. With both, the thread wins, since "answer this question" is the common shape.
- **`--thread-id <UUID>`**: the originating thread. With `--tap navigate`, the tap opens this thread instead of the inbox modal. Even without `--tap`, it stamps the notification so the modal's "Open thread" button resolves.
- **`--event-id <UUID>`**: a specific event id inside `--thread-id` to scroll to and briefly pulse when the tap lands. Ignored when `--thread-id` is absent. An event that is not in that thread fails the command with a 400, and so does a domain event, which lives in no thread.
- **`--fragment <string>`**: the place INSIDE the app the tap lands on. It arrives as the app's `location.hash`, so an app that routes on the hash opens on that item. Only read on the `--app-id` branch, since a thread deep-link names no app. An app that ignores the hash still opens.

```bash
# Deep-link the push to the exact UserQuestionAsked card on tap.
lucidos notify \
  --title "Coding agent is asking" \
  --message "Ship it?" \
  --tap navigate \
  --thread-id "$TRIGGER_EVENT_THREAD_ID" \
  --event-id "$TRIGGER_EVENT_ID"
```

The engine sets `TRIGGER_EVENT_THREAD_ID` and `TRIGGER_EVENT_ID` on every script trigger fired by a thread-scoped event (see `triggers.md` § "Script trigger env vars"). A schedule-fired trigger gets neither, so `--tap modal` (the default) is the only meaningful choice.

An app deep-link takes `--fragment` the same way, so the tap lands on the item:

```bash
# Land on the habit the reminder is about, not on the board's default sort.
lucidos notify \
  --title "A streak is at risk" \
  --message "Hydration has no entry for today." \
  --tap navigate \
  --app-id habit-tracker \
  --fragment "habit-hydration"
```

The CLI rejects `--tap navigate` with neither `--thread-id` nor `--app-id` (it needs a destination) before the HTTP round-trip. The server returns the same 400 if that check is bypassed. Panel-shaped targets (`changes`, `triggers`, `files`, …) have no CLI flag: use the `send_notification` LLM tool, or POST to `/api/v1/notifications` with the full structured `tap` object.

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

Read and clear the notification *inbox* (`notify` only *sends*). It is generated
from the capability parity manifest, so it uses the same gateway-safe HTTP
client as every other subcommand. Use it instead of hand-rolled `curl`, which
would have to guess the engine port and the gateway `/<slug>/` path prefix.

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

> **In-thread agent:** the chat Lucidos Agent has the grouped `notifications`
> tool (`action: list | mark_read | mark_all_read`), which runs **in-process**.
> Use the tool from a chat or trigger thread. Use this CLI from a
> `script:`-typed trigger or a coding-agent, bash or Python subprocess. Both are
> checked against the same capability parity manifest, so they cannot drift.

### `lucidos preferences get | set --key <K> --value <V>`

Read and change user *preferences* (Settings). Generated from the capability
parity manifest (gateway-safe HTTP client). `get` lists every settable key with
its current value, allowed values, default, and scope; `set` changes one.

```bash
$ lucidos preferences get
$ lucidos preferences set --key timezone --value Europe/Oslo
$ lucidos preferences set --key chat_model --value claude-opus-5
```

`get` accepts `--device-id <id>` (read device-scoped overrides; omit for the
global view). `set` requires `--key` + `--value`; pass `--device-id` only for a
per-device key. The chat agent's in-process equivalent is the grouped
`preferences` tool (`action: get | set`).

### `lucidos triggers list | create | update | delete | run`

Manage *triggers*: scheduled (cron) and/or event-driven automations. Generated
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
# Pin which backend serves a model that has more than one route
$ lucidos triggers update --id <uuid> --model claude-opus-5 --provider anthropic
```

`--provider` needs a model pin, from the same request or already on the
trigger, and must name one of that model's routes. Anything else is refused at
save time. A later `--model` change clears the pin unless the same request sets
`--provider` again. At fire time, a pin to a backend with no credential refuses
the fire rather than running it elsewhere.

`--cron-expressions` entries are validated on `create` and `update`. Within one
expression the fields are ANDed and across the array they are ORed, so
`0 0 9 1 * Mon` is the 1st only when it is a Monday. An expression that can
**never** fire (`0 0 9 31 2 *`, Feb 31) is refused with an error naming the
offending fields. A successful write returns a `cron_preview` object with
`next_runs` (the next few fire times) plus any `warnings`. Read the preview
back rather than assuming the schedule means what you intended.
`system-knowhow/triggers.md` § "Writing cron expressions" has the recipes.

`create`/`update` accept `--name`, `--run`, `--cron-expressions`, `--on`,
`--app-id`, `--go-to-review`, `--group-id`, `--side-effect-grant`, `--slug`,
`--model`, `--reasoning-effort`, `--provider`;
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

Manage *trigger groups*: the folders that organize triggers in the panel. A
group is only a label and fires nothing.

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

List, inspect, rename, or delete *apps*. Creating an app and editing its source
are not CLI ops: creation is the chat agent's `create_app` tool, and source
editing happens in the app's coding-agent worktree.

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

Inspect the *Thread Queue* (background admission control). `list` prints the
live queue and the active *capacity policy* as JSON. `run-now` force-admits a
queued entry, ignoring caps. `drop` removes a queued entry without running it.

```bash
$ lucidos thread-queue list
$ lucidos thread-queue run-now --entry-id 0b1e…  # force-admit
$ lucidos thread-queue drop --entry-id 0b1e…     # cancel a queued entry
```

Get an entry id from `list` (`entries[].id`). Mirrors the chat agent's grouped
`thread_queue` tool. Changing the capacity policy is that tool's
`update_policy` action, and deliberately **not** a CLI command. The raw
`PUT /api/v1/thread-queue/policy` replaces omitted caps with defaults, while the
LLM tool merges with the live policy.

### `lucidos memory stats | entries [...] | search --q <Q> [--limit N] | source [--source-id UUID] [--source-type T] [--path P] [--commit C]`

Read long-term memory. `stats` (index counts), `entries` (paginated, with
importance and source), `search` (rank entries against a question), `source`
(the originating event or artifact for one memory, plus the entries derived
from it). All read-only.

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
tokens, passwords) use a credential, never this**: env var values appear in
logs and events. `set` is an upsert (create-or-replace).

The chat agent has the grouped `env_vars` LLM tool (`list` / `set` / `delete`).
The retired `set_environment_variable` name still works as an alias for `set`.

### `lucidos models list | add --id <id> (--provider <p> | --routes <JSON>) [--label L] [--sort-order N] [--context-window N] [--vision true|false] | update --id <id> [...] | delete --id <id>`

Manage the chat-model registry (Settings → Models): the models in the Lucidos
Agent's picker.

```bash
$ lucidos models list
$ lucidos models add --id z-ai/glm-5.2 --provider openrouter --label "GLM 5.2" \
    --context-window 1048576
$ lucidos models update --id z-ai/glm-5.2 --context-window 1048576
$ lucidos models update --id z-ai/glm-5.2 --enabled false   # disable
$ lucidos models delete --id z-ai/glm-5.2                   # user models only
# One model, two backends: Vertex first, then OpenRouter under its own id
$ lucidos models update --id claude-opus-5-5 \
    --routes '[{"provider":"vertex"},{"provider":"openrouter","id":"anthropic/claude-opus-5-5","context_window":200000}]'
# Which backend to use when more than one route is configured
$ lucidos models update --id claude-opus-5-5 --preferred-provider openrouter
# The model reads images, so image description may use it
$ lucidos models update --id z-ai/glm-5.2 --vision true
```

A provider is one of `vertex`, `anthropic`, `openai`, `openrouter`, `xai`,
`opencode-free`, `local`.

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
engine guesses from the model id: `claude-*` is 200k unless the id carries
`[1m]`, `gpt-5*` is 400k, anything else 200k. OpenRouter, xAI, Gemini and local
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

**`--vision` says whether the model reads images.** Image description lists,
defaults to and runs only such models. `add` without it means `false`, and
`update` without it keeps the stored value. Builtins ship it on the Claude,
Gemini and GPT-5/6 rows. The rest are unverified, so set it once you know.

Builtin models can be disabled (`update --enabled false`). They accept route
edits (`--routes`, `--provider`, `--context-window`), `--vision` and a
`--preferred-provider`, because which backends serve a model changes, and a
seeded flag can be wrong. They
cannot be renamed, re-sorted or deleted: their identity is engine-owned. To
change the **default** chat model for new threads, set the `chat_model`
preference instead. A running thread reuses its own last-used model (see
`preferences.md`). Mirrors the chat agent's `manage_models` tool.

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

### `lucidos changes list [--sub-threads-of <uuid> | --my-sub-threads]`

List pending, set-aside and recently-applied *changes*. Wraps `GET /api/v1/changes` and echoes the engine's payload verbatim to stdout. This is how a script finds a pending change's id before `apply`: read `.pending[].id`. Never scan `ChangeProposed` events for it.

```bash
$ lucidos changes list
{"pending":[{"id":"fbcc4a3a-...","branch_name":"lucidos-claude-code-repo-lucidos-fix-...","description":"fix: …","status":"pending",...}],"set_aside":[],"applied":[...],"total_pending":1,"restart_required":false,"restart_groups":[],"client_update_available":false,"has_more_applied":false}

# Find the single pending change's id (e.g. in a build → apply pipeline):
$ CID=$(lucidos changes list | jq -r '.pending[0].id')
$ lucidos changes apply "$CID"
```

The response carries `pending`, `set_aside` (kept for later, newest first), `applied` (recently applied), `total_pending`, and `restart_required`. Each pending change has `id` / `branch_name` / `description` / `status` / `file_count` / `requires_restart` / `thread_id` / `thread_title`. Its apply state is `thread_unsettled` / `thread_settling` / `resolving_conflict` / `apply_phase_started_at` / `predicted_conflict`. Exit non-zero on transport / HTTP error.

`predicted_conflict` is `conflict`, `clean` or `unknown`: would merging the change into `main` right now conflict. `unknown` means git could not answer, so never read it as clean. `apply_phase_started_at` is when a running hardening or conflict resolution began.

**`thread_unsettled: true` means the proposing thread is still working on the change**: mid-turn, on a question card, resolving a merge conflict, or watching an event. `apply` refuses it for exactly that reason, so never report such a change as finished. `thread_settling` is the part of that a *standing apply* can wait out. `resolving_conflict` means an apply of it is merging now.

Narrow `pending` to one subtree:

```bash
# Changes held by any sub-thread of one thread, at any depth (not its own):
$ lucidos changes list --sub-threads-of 6fa459ea-ee8a-3ca4-894e-db77e160355e
# The same, for the thread this subprocess runs in:
$ lucidos changes list --my-sub-threads
```

`--my-sub-threads` reads `LUCIDOS_THREAD_ID`, like `threads list --my-children`, so it works only from inside a Lucidos thread. `total_pending` counts the narrowed list.

> **In-thread agent:** the chat Lucidos Agent has the `changes` tool. Its `list` action returns the same `{pending, set_aside, applied, total_pending}` shape **in-process**, with the same flags filled. Its `sub_threads_of` argument takes a thread id, or `current` for its own thread. Use it from a chat or trigger thread. Use this CLI from a `script:`-typed trigger or a bash or Python subprocess.

### `lucidos changes apply <change-id>`

Apply a pending *change* (a coding-agent-proposed branch waiting on the Apply button). Wraps `POST /api/v1/changes/<id>/apply` and echoes the engine's typed `ApplyChangeResult` JSON to stdout. Get the id from `lucidos changes list` (`.pending[].id`).

```bash
$ lucidos changes apply fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec
{"status":"applied","change_id":"fbcc4a3a-...","thread_id":"1c1c34ef-...","message":"Change applied.","restart_required":false,"applied_commit":"9b1a...","previous_commit":"2a3b...","commits_applied":3,"files_changed":5}
```

> **In-thread agent:** the chat Lucidos Agent has the `apply_change` LLM tool. It runs the same apply pipeline **in-process**. It stamps the apply as the agent, linked to the applying thread, so the route popover never mislabels it as "You". Use `apply_change` from a chat or trigger thread. Use this CLI from a `script:`-typed trigger or a bash or Python subprocess, which cannot call the in-process tool.

> **Both refusals below reach the LLM tool too.** `apply_change` asks the same gate this CLI's route asks (ADR 0233). A thread that has not settled, or a change with no files left, is refused there as well. The tool error names the way forward. It offers `apply_when_settled` only for a *settling* thread, because `apply_when_settled` refuses one parked on a question.

> **Applying work that has not finished:** the agent also has two *standing apply* actions. `apply_when_settled` takes one thread's change, applied the moment that thread finishes. `apply_as_they_settle` takes everything pending that has settled, plus every *settling* thread as each one lands. Both arm the same instruction the Apply control arms from the UI. Both wait through an event wait, and drop with a report if a thread stops on a question or fails. LLM-only, with no CLI form.

> **Nothing to wait for, nothing armed.** `apply_when_settled` refuses a thread that has already settled with nothing pending, parked on a question, or failed. So never re-arm a thread whose change just landed.

> **Neither reaches a repo Lucidos does not apply into.** An *external-repo coding-agent thread* proposes no *change* at all, so `apply_when_settled` refuses one and the `apply_as_they_settle` sweep passes it over. That work is reviewed and pushed from the repo itself.

> **Taking one back:** `cancel_standing_apply` is the off for both. With a `thread_id` it cancels that thread's instruction. Without one it cancels every standing apply in the workspace. The Changes panel's Not finished toggle cancels only the changes it lists. It stops future applies only: a change already merging or hardening keeps going, and nothing already applied is reverted. Cancelling a running Apply All batch is a different action.

> **Setting a change aside:** `set_aside` keeps a pending change for later, out of Review and Apply All, and lets its thread be archived. `bring_back` returns it to pending. Neither loses the branch. `set_aside` is refused where Discard is, and `apply` refuses a set-aside change until it is brought back. LLM-only, with no CLI form.

> **All six ask the same authority question the CLI does.** Applying a change from your own subtree needs nothing. Anything wider is the *workspace owner*'s, and the tool returns the refusal above rather than applying. `apply_as_they_settle` is always wider, since the sweep reaches every thread in the workspace, and so is a `cancel_standing_apply` naming no thread.

The response carries:

| Field | Meaning |
|---|---|
| `status` | `applied`, `noop`, `hardening`, or `conflict` (see `docs/apply-change-api.md` for the full table) |
| `applied_commit` | 40-char SHA on `main` AFTER the merge (present on `applied` and idempotent `noop`) |
| `previous_commit` | 40-char SHA on `main` BEFORE the merge |
| `commits_applied` | Number of commits added to `main` (0 for `noop`) |
| `restart_required` | `true` when the change needs a new engine version. Apply never restarts Lucidos: it builds the new version in the background, and the user taps "Switch to new version" to restart onto it. Never tell them an apply restarts Lucidos. |
| `conflict_thread_id` / `review_thread_id` | Thread to focus when `status` is `conflict` / `hardening` |

The CLI prints the JSON verbatim on stdout. It exits non-zero on a transport error or a 4xx, with the engine's error body on stderr, like `lucidos proxy --fail`.

Two 409s are refusals rather than errors, and both name the resolution:

- The change's thread is still working: wait for it to idle.
- The change has **no file changes left** (`file_count` is 0). Its branch's commits cancelled out, so there is nothing to merge: discard it with the Discard button. A build-then-apply script treats a zero-`file_count` entry in `lucidos changes list` as "nothing to apply", not a change to retry.

A 403 is the third refusal, about authority rather than state. Applying a change acts on the thread that proposed it, so a change from your own subtree needs nothing. A change from anywhere else is the *workspace owner*'s to apply. You may apply it only with their standing instruction: a turn they opened, or a trigger firing they authorized. Discard answers the same way. The body names both shapes, so read it rather than retrying.

#### Why the CLI and not hand-rolled urllib / curl

A hand-rolled request gets two things wrong that the script cannot see. And the
rule at the top of this file forbids raw HTTP to the engine at all.

**It loses your identity.** The CLI forwards the subprocess-origin header
(`x-lucidos-agent-origin-token`). The engine reads it to stamp the
`ChangeApplied` event as `Api { mode: Agent, source_thread_id }`. The token is
*thread-bound*: the engine mints one per spawn and reads the spawning thread off
the token, so the popover links back to the thread that acted. Without it the
engine sees an unattributed API client. Your agent action is recorded as an
anonymous one, and on the chat path it is refused outright. A `run_python` block
calling `urllib.request.urlopen(".../api/v1/changes/<id>/apply")` hits this,
because urllib does not read the env var.

**It can reach the wrong engine.** The CLI resolves this workspace's engine and
asserts its workspace, so a wrong port comes back as a 409 naming the right
one. A hand-built `https://localhost:<port>/...` asserts nothing, and whichever
engine holds that port serves it in full.

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
substitute raw HTTP (see the top of this file). Only test harnesses and
external tools that cannot shell out speak to `/api/v1` directly, and neither is
an agent working around a refusal.

If you need the underlying URL for an operation the CLI covers, use
`$LUCIDOS_API_BASE_URL`, never one built from `$LUCIDOS_API_PORT`. See
§ "Workspace resolution" for why. See `docs/apply-change-api.md` for the apply
response shape and the full workflow.

### `lucidos hardened mark` / `lucidos hardened query` / `lucidos hardened sha`

Record or read the hardening marker (see *Hardening* in the glossary): the HEAD SHA the last `/harden` run covered on a *Lucidos-source* coding-agent branch. All three resolve repo_root and branch from `$PWD`'s git worktree. They wrap `POST /api/v1/internal/mark-hardened` and `GET /api/v1/internal/hardened-state`. `mark` must run from a Lucidos-spawned process (a coding-agent session or an engine-spawned script) or from a shell on the machine holding the machine-local token. The engine refuses anyone else with 403, including a request that came through the gateway.

```bash
lucidos hardened mark    # /harden Phase 5: record HEAD as hardened
lucidos hardened query   # FRESH (HEAD matches), STALE (commits since), or MISSING
lucidos hardened sha     # the recorded SHA, fresh or stale; exit 1 when MISSING
```

Run from a coding-agent session, `mark` also stops every background task the thread still has running, since this hardening supersedes it. Their completions will not re-open the thread (ADR 0369). It then prints what it stopped, and every wait still live on the thread:

```text
Hardening recorded: my-branch 1a2b3c4d5e6f
Stopped background task 5f0c2e1a-… (make lint): this hardening supersedes it, and its completion will not re-open this thread.
Still waiting on: the release build to finish. Ending your turn leaves this thread waiting, and Apply stays withheld until that resolves. …
```

A wait still live means the session is not finished: say so, or stand it down with `lucidos event-waits cancel`. From your own shell, `mark` stops nothing.

`query` prints exactly one of the three words, because `/harden` Phase 0 and the `pre-push.sh` hook compare it literally. `sha` feeds `/harden`'s merge-only check: is every commit since that SHA a merge of main? Apply consumes the marker, so a freshly applied branch reads `MISSING`. `/harden` drives these, so you rarely call them by hand.

### `lucidos planned mark (--plan <path> | --simple "<reason>" | --security-fix "<reason>" --files <csv>)` / `lucidos planned approve` / `lucidos planned state`

Record, approve, or query the *plan marker*. Before a *Lucidos-source* coding-agent branch is edited and applied, it proves the `implementation-plan` skill ran. It also proves the human approved its plan, or that a local fix was acknowledged. Without a **gate-satisfying** marker, Claude Code's first source edit is blocked (the `cc-plan-gate` PreToolUse hook) and Apply is refused (the engine's plan floor). Wraps `POST /api/v1/internal/mark-planned` / `POST /api/v1/internal/approve-plan` / `GET /api/v1/internal/planned-state`.

`mark` and `approve` accept the same callers as `hardened mark`, and refuse anyone else with 403.

```bash
# Complex work: the implementation-plan skill writes the plan, then records this for you.
# This records the AWAITING-APPROVAL `proposed` state. It does NOT unblock editing:
lucidos planned mark --plan docs/plans/2026-06-19-my-change.md

# Present the plan, then ask for approval with your question tool (see below).
# Once the user APPROVES, flip it to gate-satisfying:
lucidos planned approve

# Genuinely local fix that needs no plan: acknowledge instead (no approval needed):
lucidos planned mark --simple "rename a misspelled variable"

# UNATTENDED run only (the nightly security pass): a security fix confined to named
# files, with a regression test. No approval step, and Apply refuses the branch if it
# touched anything outside the list:
lucidos planned mark --security-fix "unscoped local proxy key; proxy_tests::refuses_foreign_host" \
  --files crates/lucidos-engine/src/api/proxy_builtin.rs,crates/lucidos-engine/src/api/proxy_tests.rs

# Inspect the current branch's marker (SATISFIED, PROPOSED, or MISSING):
lucidos planned state
```

`mark` / `approve` resolve repo_root / branch / HEAD from `$PWD`'s git worktree (like `lucidos hardened mark`). Pass exactly one of `--plan` / `--simple` / `--security-fix`.

**`mark --plan` records `proposed` (awaiting approval). It does NOT satisfy the gate.** The agent summarizes the plan in its message and asks for approval **with its question tool**, never in prose. That is `AskUserQuestion` on Claude Code and `ask_user_question` on Codex, with options `Approve` / `Request changes`. Approval is a DECISION the agent is blocked on. Asked in prose, it leaves the thread idle until the user types "approve" by hand.

That option pair is a **floor**, not a fixed shape. The question tool needs at least two options, so `Request changes` fills the second slot only when the plan has no real fork. When it has one (a narrower scope, one layer instead of two), the fork takes the slot. `Request changes` is then dropped, not carried as a third option meaning only "I will type what I want changed".

Only after the user approves does the agent run `lucidos planned approve`, flipping `proposed` to `planned` (gate-satisfying). A fork answer is an approval too: revise the plan file to that variant, re-commit, then flip it. If the user requests changes, revise the plan file, re-commit, and ask again (the marker stays `proposed`). The message carries the plan summary, or on a re-ask only what changed. The card asks one short question and never repeats the message. `mark --simple` records `acknowledged_simple` directly, since local fixes need no approval.

`mark --security-fix` is the **bounded security-fix lane**, for one case: a run nobody can be asked. An UNATTENDED session may commit a security fix with no prior plan decision. The fix must stay inside the files `--files` names, and ship a regression test. It records `bounded_security_fix`, which satisfies the gate at once.

The lane is a distinct state on purpose. It claims neither that the work is local nor that a human approved, only that an unattended run bounded itself. **Apply refuses the branch if it touched anything outside the list**, `docs/plans/` excepted. If the fix has to grow, re-run the command with the full list. The engine caps the list and refuses a mark with no bound. If you can ask the user, ask: the lane is for security work only, not a general way past the gate.

Anything wider stays gated. The session commits its plan, leaves the marker `proposed`, and reports that it is blocked on a decision. See ADR 0154.

`planned`, `acknowledged_simple` and `bounded_security_fix` satisfy every gate. `proposed` and a missing marker both block. App coding-agent threads and external repos are exempt (the gate is a no-op there). The `implementation-plan` skill drives `mark --plan` / `approve`, and `mark --simple` is the agent's escape hatch for a change too small to plan. `lucidos cc-plan-gate` is the hidden PreToolUse hook that enforces this, never invoked directly.

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

**Why the CLI rather than starting `vite` yourself:** when a coding-agent turn ends, the engine kills the session's whole process group, and a dev server the agent started dies with it. The engine owns the process so the preview outlives the turn.

**One slot per workspace.** `start` on another thread moves the preview rather than adding a second one.

`start` refuses, by name, in four cases:

- the thread has no worktree;
- the worktree is not a Lucidos-source one (an app or external-repo thread has no frontend to preview);
- its dependencies were never provisioned;
- no workspace gateway launched the engine. It answers only once Vite is actually serving, so the printed URL is live when you paste it into a reply.

**The printed URL uses the host the CLI reached the engine on**, which from inside the worktree is `localhost`. That is right for the host machine and wrong for a phone. The in-app control (the coding-agent control menu's *Frontend preview* section) builds the URL from the page's own location, so it resolves on a tailnet. Point the user at that control rather than pasting a `localhost` URL. It also carries the device id, which the CLI cannot know. Without it the preview renders with none of that device's scoped preferences.

The preview registers **no service worker** and cannot do push: a dev server emits unhashed module URLs a worker would cache past a hot update.

**The preview needs a paired device.** It reaches the engine through the workspace gateway, and serves its own files only to a browser holding the gateway's device cookie. So it opens in a browser that already has Lucidos open, and answers 401 to anything else, a `curl` included. See ADR 0055 and ADR 0267.

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

**Why this exists.** The chat Lucidos Agent loads `system-knowhow/*.md` with its in-process `load_knowhow` tool, using the same id. An *app coding-agent thread* runs in a sparse-checkout *worktree* narrowed to one `data/apps/<id>/` folder, with neither the files nor that tool. This subcommand is how such a session (Claude Code or Codex) pulls the same guidance on demand:

- `system-knowhow/building-an-app`: when an app is the right answer, scaffolding defaults, common mistakes.
- `system-knowhow/js-sdk`: the `lucidos.*` SDK surface.
- `system-knowhow/best-practices`: file layout, where app data lives.

Load the relevant knowhow before writing app code, rather than guessing at the SDK surface or data paths. Any bash or Python subprocess without in-process tools uses this CLI too.

### `lucidos proxy <name> [path] [-X METHOD] [-H "Hdr: val"] [-d body | --data-stdin] [-i] [--fail]`

Call a backend through the engine: an entry in `data/config/apis.json`, or a builtin provider proxy (below). The engine resolves the credential from the workspace's credential store and injects the configured auth header. It strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request. It strips every `x-lucidos-*` header and the two `x-forwarded-*` ones the gateway owns with them, so no Lucidos credential reaches the upstream. **The credential value never reaches the script**: not in `argv`, env vars, the request line, nor any log.

The response comes back with an allowlisted set of upstream headers, the same for every caller. `Set-Cookie` and the other headers a browser acts on never arrive, so `--include` cannot read a login cookie. A cookie login belongs in a `script_handshake`, which makes its own requests. The full list is in `system-knowhow/js-sdk.md` § `lucidos.proxy`.

**This is the preferred way for scripts to call external APIs.** The old pattern, `curl -H "Authorization: Bearer $CRED_FOO" ...` with `$CRED_FOO` in the script's environment, leaks the secret into process args and shell history. Configure the API in `data/config/apis.json` once, then use `lucidos proxy` everywhere.

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

`auth.type` selects how the engine attaches the credential to the outgoing request. Six modes:

- **`bearer`**: `Authorization: Bearer <auth_value>`. `{"type": "bearer", "credential": "<service_name>"}`
- **`api_key`**: `<header>: <auth_value>` (default header `Authorization`). `{"type": "api_key", "credential": "<service_name>", "header": "X-API-Key"}`
- **`basic`**: `Authorization: Basic <base64(auth_value)>`. The credential's `auth_value` should already be `user:password`. `{"type": "basic", "credential": "<service_name>"}`
- **`query_param`**: appends `?<param_name>=<auth_value>` to the request URL, for APIs (e.g. Helius) that take the key as a query parameter. `{"type": "query_param", "credential": "<service_name>", "param_name": "api-key"}`
- **`hmac_signed`**: signs each request with HMAC over the query string, for APIs (e.g. Binance) that require per-request signing. `{"type": "hmac_signed", "key_credential": "binance-key", "secret_credential": "binance-secret", "key_header": "X-MBX-APIKEY", "algorithm": "sha256", "signed_payload": "query_string", "signature_param": "signature", "timestamp_param": "timestamp"}`
  - Optional `timestamp_param` injects the current millis-since-epoch as a query parameter before signing.
  - Optional `key_header` (default `X-API-KEY`) carries the API key.
  - `signature_param` (default `signature`) names the resulting signature parameter.
  - `algorithm` is `sha256` or `sha512`. `signed_payload` is `query_string`.
- **`script_handshake`**: for APIs that need a multi-step login (POST creds, get a session token or multi-header response, refresh on a schedule). `{"type": "script_handshake", "credential": "<service_name>", "script": "scripts/auth/<api>.py"}`. See `system-knowhow/building-an-auth-handshake.md` for the full guide and worked Comfort Cloud + Firebase examples.
  - The engine runs a per-API Python script you write under `data/scripts/auth/<api>.py`. It caches the resulting headers in memory, and refreshes on `expires_in` or an upstream 401.
  - The credential can be any type. The script reads it as `CRED_<NAME>_USERNAME` + `CRED_<NAME>_PASSWORD` for `password`, or `CRED_<NAME>` for the others, as `run_python` / `run_bash` do.
  - Optional `"oauth_providers": ["google", ...]` injects each listed provider's connected access token (auto-refreshed) as `OAUTH_<UPPER>_ACCESS_TOKEN` in the script's env. A missing provider is a 502 naming it, so the user knows which `connect_oauth_account` to run.

`auth.credential` (single-credential modes) and `auth.credentials` / `auth.key_credential` / `auth.secret_credential` (multi-credential modes) name `service_name`s already in the engine credential store, the one `request_credential` writes to. An entry with no `auth` block forwards unauthenticated, for local services like Sonos.

**A credential goes only where it is scoped.** The engine checks the entry's `base_url` against the *credential scope*, the set of base URLs the credential declares, before attaching it. Rewriting an entry's `base_url` therefore cannot redirect a secret (ADR 0144). One key covering several hostnames of one provider declares each of them, and `lucidos credentials` below is how you read and set that. A `script_handshake` credential is the exception, because the script presents it rather than the proxy: what binds there is the `injects` column of the approvals record, which pins the exact secret set that entry may hand the script.

The engine also validates the upstream's certificate, and refuses a credential over plain `http://` unless the host is loopback. A self-signed dev backend or a keyed LAN device needs `"insecure_transport": true` on the entry, which is documented in `system-knowhow/building-an-auth-handshake.md`.

#### Builtin provider proxies (no `apis.json` entry)

Every model provider the engine holds auth for is also a proxy, with no `apis.json` entry and no credential to request. The names are `anthropic`, `local`, `openai`, `openrouter`, `typesafe`, `vertex` and `xai`. The engine injects the key it already uses for chat. So never ask the user for a key these already cover.

**Each default base URL already includes `/v1`.** Send the path after it. A `local` base you configure may lack it, and the agent's context says which. `/v1/models` against `openai` reaches `https://api.openai.com/v1/v1/models` and answers 404.

```bash
# List the OpenAI models the engine's own key can reach
lucidos proxy openai /models

# A chat call through the builtin Anthropic proxy
lucidos proxy anthropic /messages -X POST \
  -H "Content-Type: application/json" -H "anthropic-version: 2023-06-01" \
  -d '{"model":"<model-id>","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}'
```

- **Not configured answers 404**, and the message names what to set (Settings → Models → Providers, or an env var).
- **An `apis.json` entry with the same name wins**, so you can still point `openai` at a gateway.
- **`vertex` takes only the suffix**, `/publishers/<publisher>/models/<model>:<method>`. The engine owns the project and region prefix.
- **`opencode-free` has no proxy.** The keyless free tier serves chat only, so nothing may build on it (ADR 0104).
- The Lucidos Agent reaches the same proxies with `proxy_request`, and its context lists each one with its base and whether it is configured.
- **A model call through the proxy records what it cost.** A builtin proxy, or an `apis.json` entry pointing at a model provider's host, writes a `ContextCaptured` with `purpose: "proxy"` from the usage block the reply carries. It lands on the thread that ran the script, else on the home thread. So a script never reports its own model spend, and the Token Cost app counts it like any other call. The engine asks a model provider for an uncompressed reply, so it can read that usage block.

The full table of bases and injected headers is in `system-knowhow/js-sdk.md` § `lucidos.proxy`.

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

# Exit non-zero on HTTP errors and suppress body (curl --fail), for scripts
# that need to react to upstream failures
lucidos proxy sonos /zones --fail
```

Output is the response body on **stdout**. With `--include`, the status line and headers come first on stdout (curl convention: one stream). With `--fail`, the body is suppressed and a one-line `lucidos proxy: HTTP <code>` summary goes to stderr instead. Transport errors (DNS failure, connection refused, …) print to stderr (`lucidos: ...`) and exit non-zero. Exit codes mirror curl: `0` on success (including 4xx/5xx by default), `22` when `--fail` and the response is 4xx/5xx, `1` on transport failure.

#### Timeouts

The engine waits **30 seconds** on the upstream by default, then answers `504 upstream timeout`. The wait covers the whole reply: the proxy reads a streamed body in full before it answers, so a long streamed model call is cut at the same point. Two settings raise it, and both accept 1 to 600 seconds:

| Setting | Where | Applies to |
|---|---|---|
| `proxy_timeout_secs` | a workspace preference | every proxied call, including the builtin model routes (`vertex`, `openai`, …) that have no `apis.json` entry |
| `timeout_secs` | a field on one `apis.json` entry | that entry only, and it wins over `proxy_timeout_secs` |

```bash
# Let every proxied call wait up to five minutes
lucidos preferences set --key proxy_timeout_secs --value 300
```

```json
{ "slow-model": { "base_url": "https://llm.example", "timeout_secs": 300 } }
```

A value outside 1 to 600 is refused. For the preference, the write fails and names the key and the range. For an entry, the engine rejects that entry by name at boot, and every call to it answers 502 with the reason. Each upstream request gets the whole wait, so a redirect hop or the one retry after a 401 starts its own. One proxied call is capped at 600 seconds in total, and past that it answers 504. `lucidos proxy` itself waits a little longer, so it never gives up before the engine does.

A `script_handshake` proxy looks the same to the caller (`lucidos proxy comfort-cloud /devices/list`). The engine runs the login script and attaches the resulting headers. See `system-knowhow/building-an-auth-handshake.md` for authoring the script.

#### When to use which

| Want to … | Use |
|---|---|
| Call a backend the workspace will reuse | `lucidos proxy` (configure once in `apis.json`, then no auth in script) |
| Call a model provider (OpenAI, Anthropic, OpenRouter, xAI, Vertex, local, TypeSafe) | `lucidos proxy <builtin> <path>`, no `apis.json` entry and no credential request |
| One-off `curl` to a service the workspace will never reuse | Plain `curl` (no proxy entry needed) |
| Emit a domain event, or query the event store (domain AND engine events) | `lucidos events …` |
| Write a file under `data/` | `lucidos data write …` |
| Push a notification to the user from a script | `lucidos notify --title … --message …` |
| Find a pending change's id from a script | `lucidos changes list` (read `.pending[].id`; don't scan `ChangeProposed` events) |
| Apply a coding-agent-proposed change from a script | `lucidos changes apply <id>` (never hand-roll the HTTP call: the actor stamps as "You") |

A script doing `curl -H "Authorization: Bearer $CRED_..."` against an API the workspace holds a credential for is drift. Add an `apis.json` entry and switch the script to `lucidos proxy`.

### `lucidos handshake list` / `lucidos handshake approve <path>`

The engine runs a `script_handshake` script only when it recorded who wrote it
(ADR 0144). `data/scripts/` is writable over the API, and an app UI reaches that
API with the user's authority, so the file existing is not enough.

The Lucidos Agent's file tools record as they write, so a script the agent
wrote or edited needs nothing more. This command is for a script that arrived
another way: the Files panel, an editor, or a plugin install.

```bash
lucidos handshake list
# approved     data/scripts/auth/firebase.py
# NOT APPROVED data/scripts/auth/comfort-cloud.py

lucidos handshake approve scripts/auth/comfort-cloud.py
# Approved data/scripts/auth/comfort-cloud.py
```

`approve` records the file as it stands now, so it has to run after the edit.
Any spelling of the path works: the `apis.json` value, the workspace-relative
form, or an absolute path inside the workspace.

**There is no button for this, deliberately.** The route refuses a
browser-shaped caller. An app UI shares the shell's origin, so a button would
let it approve a script it wrote itself. Revoking is a line deleted from
`<workspace>/.lucidos/approved-handshake-scripts`.

Each line there reads `<sha256>  <base_url>  <injects>  <path>`. `base_url` is
the one host the script's minted token may be sent to. `injects` is the set of
secrets `apis.json` may hand it, written `c:<credential>` and `o:<provider>`.
`-` means that column is not bound yet.

The first proxy call binds both from `apis.json`, and a later rewrite of either
is refused. Edit a column by hand to move a script, to share it with a second
provider, or to change which secret it receives.

### `lucidos credentials list [--name <N>] [--json]` / `lucidos credentials set-base-urls --name <N> [--auth-type <T>] --url <U> ...`

Read and set the *credential scope*: the base URLs one stored credential may be
presented to. The proxy sends a credential nowhere else, so this is the setting
behind a `will not be sent to` 502.

**A scope is a SET, because one key often covers several hostnames.** Binance
signs spot calls at `api.binance.com` and futures calls at `fapi.binance.com`
with the same HMAC pair, and Helius issues one key for two hosts. Each host is
named exactly. There is no wildcard, and nothing is inferred from a host's
spelling (ADR 0161).

```bash
lucidos credentials list
# binance-key              api_key          https://api.binance.com
# google                   oauth_client     https://oauth2.googleapis.com
# webhook-secret           secret           (no base URL, so it is sent nowhere)

lucidos credentials set-base-urls --name binance-key \
  --url https://api.binance.com --url https://fapi.binance.com
# binance-key now covers https://api.binance.com, https://fapi.binance.com
```

`set-base-urls` **replaces** the whole set, so pass every host the credential
should reach. To widen one, run `list`, then `set-base-urls` with the old hosts
plus the new one.

`--json` prints the engine's own array, for a script that wants to read the set:

```bash
lucidos credentials list --name binance-key --json | jq -r '.[0].base_urls[]'
```

`--auth-type` picks a row when an OAuth client registration shares a name with
an API key. Omit it unless the command asks; a name is otherwise unique.

Each URL needs its scheme, so `https://api.example.com` rather than
`api.example.com`. A value with no host is refused here rather than silently at
the proxy gate.

**Adding, rotating and deleting a credential are not here.** The secret itself
belongs in Settings (or the agent's `request_credential`), which is where a
person enters it. This command reads and moves the scope only.

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
one from **Settings → Access → Add a device**, which mints the same code and
shows the same QR, with no terminal step. `lucidos` is on no `PATH` either: a
desktop install keeps it at `Lucidos.app/Contents/Resources/lucidos`, and a
headless one under the install prefix in `runtime/current/`.

**`--qr` needs an address the phone can reach.** This command talks to
`127.0.0.1`, and a QR aimed there helps nobody. So it resolves a hostname from
the interface list: the MagicDNS name, else the tailnet address, else whatever
`--host` says (which implies `--qr`). Tailscale is only picking a name to
print, and the auth decision reads none of it.

**Then it knocks on the door.** Holding a tailnet address does not make it
reachable. The packaged gateway binds loopback, where `<name>:<port>` is dead.
`tailscale serve` fronts 443 on that same name and answers where the gateway's
own port does not.

So both origins are probed, and the first that answers wins. With neither
answering there is no QR, and the command says which knob to turn. An explicit
`--host` is never refused: probing only picks which of its two origins to use.

The QR is drawn black on white, because a dark terminal would invert it and
many scanners refuse that. `NO_COLOR` drops the escapes.

**A browser has to pair too, even on that same machine.** Proving you are local
means reading a file only your user can read, and a browser cannot read files.
So a browser on the host uses the same code as a phone.

Only a process on that machine, or an already-paired device, can mint a code.
That stops a remote caller from pairing itself in.

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

**A signed webhook gets no token.** Configure `--hmac` and the hook
authenticates by signature alone (see § "Changing what a hook verifies with").

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

#### Where the shared secret comes from

Which side invents it depends on the sender, so `--signing-secret` saves the
credential in the same call rather than making you save it first:

```bash
# GitHub lets you choose the secret. Lucidos mints it and prints it ONCE.
lucidos webhooks create --name github --event-type PullRequestOpened \
  --hmac '{"credential":"github-deploys",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}' \
  --signing-secret '{"mode":"generate"}'
```

Paste the printed value into the Secret field on GitHub's own webhook form.

**Slack and Stripe issue their own**, shown on the app's Basic Information page
and on the endpoint's dashboard page. A secret you invent could never verify
their deliveries, so those take `{"mode":"provided","value":"..."}`.

A provided value is stored byte for byte. One that starts or ends with
whitespace is refused by name, rather than trimmed: trimming can break a value
that needs it, and keeping it breaks every delivery with nothing saying why.

On `create` a name that already exists is refused, so nothing you saved earlier
is overwritten by accident. On `update` it replaces the value, which is how you
rotate.

#### Changing what a hook verifies with

`--hmac` on `update` takes the same object, and the hook keeps its delivery URL.
Delete and recreate would change the URL and break the sender.

```bash
# Fix a wrong signature header, or point at a different credential.
lucidos webhooks update --id <uuid> \
  --hmac '{"credential":"github-deploys",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}'

# Rotate the secret and touch nothing else.
lucidos webhooks update --id <uuid> --signing-secret '{"mode":"generate"}'

# Stop signing. Prints a bearer token ONCE.
lucidos webhooks update --id <uuid> --hmac null
```

**A hook carries exactly one verifier kind**, so each of those moves the other
one too. Adding a signature drops any token, because a sender that signs
attaches no bearer token and a hook holding both would refuse every delivery.
Removing one mints a token, because a hook with no verifier at all cannot be
stored.

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

Senders resend. GitHub retries a slow response and has a Redeliver button, and
Stripe retries for days. By default Lucidos emits on every arrival, so a resend
fires your triggers twice. That default is a real choice: every arrival stays on
the log, so with the delivery-id header allow-listed, a script trigger can count
how often a sender resends.

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

## Workspace resolution

The CLI finds **which workspace** to talk to in this order:

1. **`$LUCIDOS_WORKSPACE`**. The engine sets it on every spawned subprocess (Python, bash, coding-agent sessions), so it is authoritative there.
2. **Walk up from `$PWD`** to the first ancestor directory holding a `.lucidos/ports` file. The fallback for a terminal user running the CLI by hand without the env var.

**Reaching the engine API.** Once the workspace is found, the CLI picks the engine's base URL in this order:

1. **`$LUCIDOS_API_BASE_URL`**: set by the engine on every spawned subprocess when it is reachable somewhere other than the ports-file port. It is the exact base this engine answers on: loopback `http://` under the gateway, `https://` self-signed in the legacy single-engine model.
   - Under the workspace gateway (ADR 0014) the engine binds a **loopback HTTP** port. The user-facing port belongs to the gateway, which routes the workspace under `/<slug>/`.
   - So a bare `https://localhost:<gateway-port>/api/v1/...` request never reaches the engine: the gateway reads the first path segment as a workspace slug. Never build a URL from `$LUCIDOS_API_PORT`.
2. **`.lucidos/ports`** (`API_PORT` + optional `PROTO`, default `https`): the legacy single-engine model, where the engine listens directly on the user-facing port. Used when `$LUCIDOS_API_BASE_URL` is absent (legacy, Tauri, terminal).

## Common patterns

### Write an artifact and emit a completion event

The canonical end of an analysis or report session. An analysis app's prompt uses this instead of writing into the worktree:

```bash
ARTIFACT="artifacts/data-analysis/$(date +%Y-%m-%d)/report.html"

# 1. Write the artifact under data/.
lucidos data write "$ARTIFACT" --from /tmp/report.html

# 2. Tell the workspace it's ready.
lucidos events emit AnalysisCompleted \
  --summary "UA analysis for $(date +%Y-%m-%d) finished" \
  --payload "{\"artifact\": \"$ARTIFACT\"}"
```

Both calls use the same `data/`-rooted path, so a frontend SSE listener calling `lucidos.data.url(payload.artifact)` resolves the link.

### Call an external API and persist the response

The canonical "pull from a service, store under `artifacts/imported/`, signal completion" loop. The engine handles auth, so the script never sees the credential.

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

Compare the pre-proxy form, which leaks the credential into argv and shell history:

```bash
# Don't do this: $CRED_COMFORT shows up in `ps`, in shell history, and
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

1. Symlinks the bundled `lucidos` binary into `<workspace>/.lucidos/bin/lucidos` (idempotent, so safe on every spawn).
2. Prepends `<workspace>/.lucidos/bin` to `PATH`.
3. Sets `LUCIDOS_WORKSPACE=<workspace>` so the CLI's fallback always resolves.

For Claude Code sessions it also writes a skill file at `<worktree>/.claude/skills/lucidos-cli/SKILL.md`, so Claude Code discovers the CLI through its normal skill mechanism. Codex sessions get the CLI guidance in their system prompt. A one-line reminder ("Use the `lucidos` CLI for any data-dir writes or event emits.") in a trigger or app prompt still helps.

## Implementation

- Source: `crates/lucidos-cli/`
- Shared engine wiring: `crates/lucidos-engine/src/runtime/lucidos_cli.rs`. `lucidos_cli_dir` discovers the binary, `ensure_workspace_bin_symlink` installs the workspace-relative symlink, `workspace_script_env_vars` builds the env var bundle.
- Used by `claude_code.rs` (Claude Code sessions) and `build_script_env_vars` in `crates/lucidos-engine/src/engine/engine_impl/scripts.rs` (Python and bash tool calls, scheduled scripts).
- For the in-browser equivalent used by app UIs, see [`js-sdk.md`](./js-sdk.md).
