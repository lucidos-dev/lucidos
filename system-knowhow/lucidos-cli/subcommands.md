---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

<!-- domain-event: AnalysisCompleted: Example domain event emitted by an analysis script. -->
<!-- domain-event: DataImported: Example domain event emitted by a data-import script. -->

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

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

### Hidden: `lucidos cc-question-guard`

The PreToolUse hook Claude Code runs before every tool call in a Lucidos
session. While the agent's own question is open, it refuses every other tool
call with the reason the engine passes in `--refusal`. The answer comes back as
the question tool's result, and the agent makes the call again after that.

It reads the session transcript, not the engine. A question is open from its
`tool_use` until its `tool_result`, or until the owner's next prompt starts a
new turn. Claude Code writes each block before it runs that block's tool, so a
sibling call streamed after the question always finds it. The engine names the
question tools with `--question-tool`. The hook fails open on any read or
parse error. Codex has no such hook: only its commands and file changes, which
ask the engine first, are refused while its question is open.

### Hidden: `lucidos cc-bash-guard`

The PreToolUse hook Claude Code runs before every `Bash` call in a Lucidos
session. It checks standing denies with the engine before applying two local rules:

- **Broad kills.** `ps` or `pgrep` output fed to `kill`, `pkill -f`, and any
  kill aimed at `claude`. A Claude Code process's argv names tools like
  `cargo`, so an argv match kills every concurrent session.
- **A `cd` with an output redirection** (`>`, `>>`, `&>`, `tee`). Claude
  Code's own safety check stops every such command for manual approval,
  whatever the allow rules say. The agent rewrites it with absolute paths, so
  the user never sees that permission card.

The local rules read the command as text, so a quoted mention can trip them.
Inside Lucidos, an unreadable payload or an unverified standing deny decision
refuses the call. Both hooks retry connection refusals for up to three seconds.
If the refusal says the engine may be restarting, wait a few seconds and run
the command again. The hook runs automatically.

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
refreshes live, and memory picks the file up under either *memory module*. An
`on_event: ArtifactCreated` trigger sees it, and the chat link below resolves.

Content is limited to 100 MiB per write. A failed write exits non-zero, prints
the engine's reason on stderr, and prints no link.

The engine waits up to 20 seconds for the workspace repo lock. A
`workspace_repo_busy` refusal (HTTP 503) leaves the file untouched, so running
the command again is safe. The CLI stops waiting for the response after 60
seconds. On that timeout the engine may still finish the write.

The engine checks a tree's rules before anything reaches disk. A file
under `themes/` must be a valid theme (`system-knowhow/themes/switch-themes.md` § "Make a
theme"). A file under `fonts/` must be a workspace font's `font.json` or one of
its font files (`system-knowhow/workspace-fonts.md`). A refused write exits
non-zero with the reason and writes nothing.

A write that leaves a knowhow file over 24 KB still lands. It prints a warning
on stderr, before the path line, naming the size and the fix: split the doc
into a short index plus parts (`system-knowhow/building-knowhow` § Size).

Under the Codex sandbox this works without a writable-root grant, because it is
an HTTP call rather than a write outside the worktree.

```bash
# from a local file you generated elsewhere
$ lucidos data write artifacts/data-analysis/2026-04-20/report.html --from "${TMPDIR:-/tmp}/report.html"

# from stdin (default; also `--from -`)
$ echo '{"hello": "world"}' | lucidos data write artifacts/foo.json
```

Two outputs:

- **stderr**: the resolved absolute filesystem path, so you can capture it separately (`… 2>"${TMPDIR:-/tmp}/path"`). The path is always the last line.
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
$ lucidos data write artifacts/design/options.png --from "${TMPDIR:-/tmp}/options.png"
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

This reads the **whole** store, not only what the workspace emitted. Engine thread and system events (`ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`) are rows in the same table. See `system-knowhow/thread-events/one-table-two-enums.md` § "One table, two enums".

`--thread-id` narrows to one thread, which is how you READ a past conversation. Pair it with `--type MessageReceived` (or `ResponseGenerated`), or the query returns that thread's whole transcript, every streamed token included. Without it the query covers every thread.

With `--thread-id`, the command first prints the thread's *thread liveness* as one line on **stderr**, so stdout stays the JSON array:

```
# thread: working · running · session 2 (handoff, 9 min) · active 4 s ago
```

The newest row is history; this line is the thread's state now. `lucidos events count --thread-id` prints the same line. An engine without the liveness route prints none.

The first word is the liveness state. The second is the turn status. `idle`
as a turn status means no turn is open. The line names each live event wait
and its time left, each awaited child, and any hold or current expected wake.

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

### `lucidos events count [--type T] [--since iso] [--until iso] [--thread-id UUID]`

Count events by type/time without materialising payloads. Mirrors the `events(action="count")` LLM tool. Two shapes:

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

`--thread-id` counts one thread's events only, and a value that is not a uuid is refused. Without it the count covers every thread.

`byte_total` is `SUM(octet_length(payload::text))`, the raw payload byte sum. It is a reliable proxy for the token cost of the matching `lucidos events query`. On a busy workspace, run it before `query` to pick which types to drill into. A `query --type ToolResult --limit 300` can return 2.3 MB and blow the next turn's prompt cap.

