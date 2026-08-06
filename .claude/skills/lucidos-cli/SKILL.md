---
name: lucidos-cli
description: Use whenever you need to write a file under the parent workspace's data/ directory (artifacts/, knowhow/, apps/, triggers/), emit/query a domain event, apply a pending change (`lucidos changes apply <id>`), or spawn a new Lucidos thread (sub-thread in this workspace, or task in another workspace) from a Claude Code subprocess running in a Lucidos worktree. Prefer this CLI over Write/Edit for any path that should land at <workspace>/data/... — using Write/Edit puts the file inside the worktree, where the dev server cannot serve it and links 404. Triggers include phrases like "write to artifacts/", "write the report", "save to data/", "emit AnalysisCompleted", "emit a domain event", "query events", "send to dev", "run in another workspace", "apply the change", "apply pending change", "apply this change for me". Do NOT load this skill just because a related bug surfaced mid-task — fix it in the current thread by default; only spawn a sub-thread when it's cross-repo, cross-workspace, or the user explicitly asks for a separate thread.
---

# `lucidos` CLI — talking back to the parent workspace

You are running inside a Claude Code subprocess spawned by Lucidos, in an isolated git worktree. The worktree is *not* the workspace — anything you write with `Write`/`Edit` lands inside the worktree and is invisible to the running engine.

The `lucidos` CLI is on your `PATH`. Use it whenever you need to:

- write a file that should appear in the workspace's `data/` directory (artifacts, knowhow, app code, trigger code)
- emit a domain event back to the parent workspace
- query existing events from the parent workspace
- spawn a new Lucidos thread — a sub-thread in this workspace, or a task in another workspace

## When to use which tool

| Task | Use |
|------|-----|
| Edit source code in this worktree | `Write` / `Edit` |
| Create an artifact the user / app UI will see | `lucidos data write` |
| Tell the workspace something happened | `lucidos events emit` |
| Look up prior events | `lucidos events query` |
| Apply a pending change | `lucidos changes apply <id>` |
| Spawn a sub-thread or hand a task to another workspace | `lucidos spawn-thread` |
| Wait on something the engine will emit, without polling | `lucidos await-event` |

**This rule extends to bash and python you write.** A Python script using `open('artifacts/foo.html', 'w')` from inside the worktree has the same problem your `Write` tool has — the file lands in the worktree, not the workspace. Inside scripts, shell out to `lucidos`:

```python
import subprocess
# Resolve the absolute path the workspace expects, then write directly.
path = subprocess.run(
    ['lucidos', 'data', 'path', 'artifacts/foo/report.html', '--mkdir'],
    capture_output=True, text=True, check=True,
).stdout.strip()
open(path, 'w').write(html)
# Or pipe content through stdin:
subprocess.run(['lucidos', 'data', 'write', 'artifacts/foo.json'],
               input=json.dumps(data), text=True, check=True)
```

```bash
# In bash, just call lucidos directly — no path dance needed.
lucidos data write artifacts/report.html --from /tmp/report.html
```

## Subcommands

### `lucidos data path <relative> [--mkdir]`

Print the absolute path that `<relative>` resolves to inside the parent workspace's `data/` directory. Same normalization as the JS SDK / artifacts UI: paths starting with `artifacts/`, `knowhow/`, `apps/`, or `triggers/` are kept as-is; anything else is prefixed with `artifacts/`.

```bash
$ lucidos data path data-analysis/foo/report.html
/Users/.../workspaces/myws/data/artifacts/data-analysis/foo/report.html

$ lucidos data path knowhow/my-app/notes.md --mkdir
/Users/.../workspaces/myws/data/knowhow/my-app/notes.md
```

### `lucidos data write <relative> [--from <path> | -]`

Write content to the resolved absolute path. Creates parent directories. Reads from a local file with `--from <path>`, or from stdin (default, also `--from -`).

```bash
# from a file you generated in /tmp
$ lucidos data write artifacts/data-analysis/2026-04-20/report.html --from /tmp/report.html

# from stdin
$ echo '{"hello": "world"}' | lucidos data write artifacts/foo.json
```

### `lucidos events emit <EventType> --payload <json> [--summary <str>]`

POST a domain event to the parent workspace's event store. Event types are PascalCase past tense (`AnalysisCompleted`, `DataImported`). The payload **must** include a `summary` string — pass it via `--summary` if you don't want to inline it in the JSON.

```bash
$ lucidos events emit AnalysisCompleted \
    --summary "UA analysis for 2026-04-20 finished" \
    --payload '{"artifact": "artifacts/data-analysis/2026-04-20/report.html", "rows": 1240}'
```

### `lucidos events query [--type T] [--since iso] [--until iso] [--limit N]`

GET events. Outputs the raw JSON array on stdout — pipe through `jq` if you need to slice it.

```bash
$ lucidos events query --type AnalysisCompleted --limit 5 | jq '.[0]'
```

### `lucidos changes list`

List pending + applied *changes*. Wraps `GET /api/v1/changes`; echoes the payload verbatim. Use it to find a pending change's id before `apply` — read `.pending[].id`. Don't scan `ChangeProposed` events (`lucidos events query`) for the id; this gives it directly.

```bash
$ CID=$(lucidos changes list | jq -r '.pending[0].id')
$ lucidos changes apply "$CID"
```

### `lucidos changes apply <change-id>`

Apply a pending *change* (a CC-proposed branch waiting on the Apply button). Wraps `POST /api/v1/changes/<id>/apply`; echoes the engine's typed `ApplyChangeResult` JSON on stdout.

```bash
$ lucidos changes apply fbcc4a3a-2c14-4d5b-8d1a-9e84d4c9d4ec
{"status":"applied","change_id":"fbcc4a3a-...","applied_commit":"9b1a...","commits_applied":3, ...}
```

**Always use this instead of hand-rolling the HTTP call.** A `Bash` block that runs `curl -X POST .../api/v1/changes/<id>/apply` from inside this worktree ships without the subprocess-origin headers, so the engine stamps `Api { mode: Human }` and the UI shows the resulting Apply card as **"You"**, wrongly attributing your action to the user. The CLI forwards `x-lucidos-agent-origin-token` from the engine-injected env var, so the card correctly says "Lucidos Agent" with the source thread linked. That one header carries both facts: the token is bound to the thread it was minted for, and the engine reads the source thread off the token itself.

`status` is `applied`, `noop`, `hardening`, or `conflict`. See `docs/apply-change-api.md` for the full response shape. Exit non-zero on transport / HTTP error with the engine's error body on stderr.

### `lucidos await-event --on <EventType> [--condition <json>] --timeout-secs <n> --reason <text>`

Subscribe THIS thread to a Lucidos event, then **finish your session**. Returns
immediately and blocks nothing: the engine re-opens this thread with a follow-up
message when a matching event lands, or tells you the deadline passed. Wraps
`POST /api/v1/threads/<id>/event-waits`; `$LUCIDOS_THREAD_ID` names the thread.

```bash
# Spawn a sidequest, subscribe to its completion, and STOP.
$ lucidos spawn-thread --relation child --to "$(basename "$LUCIDOS_WORKSPACE")" --cc \
    --title "Run the e2e suite" --message "Run ./scripts/e2e.sh and report."
$ lucidos await-event --on ChildThreadCompleted --timeout-secs 5400 \
    --reason "waiting for the e2e sidequest"
```

**Use it instead of a sleep-and-recheck loop.** A `while sleep 60; do lucidos
events query ...; done` for anything the engine emits burns a tool call per
poll, samples on an interval so it can miss a transition, and dies with your
session. This does not: the subscription is persisted and survives an engine
restart.

**Then actually stop.** Say what you subscribed to and end the turn. Do not poll
for it afterwards, and do not keep the session alive waiting. The thread is
plain idle while it watches, and the wake starts a fresh turn with the whole
conversation behind it.

Not for external state with no Lucidos event (a third-party API you can only
re-query, a file another process might write): nothing would wake you, so poll
for those. A rendezvous, not a stream: the first match consumes it, and a
standing "every time X happens" rule is a *trigger* instead.

`--on` repeats to watch several types. `--condition` is a JSON object filtering
the event's own payload fields, applied to every `--on`. `--timeout-secs` is
required and capped at 24 h. `--reason` is one line the user reads in the
subscription indicator. A refusal comes back as a `400` with the reason: read it
rather than retrying.

### `lucidos spawn-thread --to <ws> [--relation child|top] [--cc] [--repo <name>] --message <text> --title <text>`

Spawn a new Lucidos thread (chat or Claude Code session). Use this when:

- The user asks you to send a task to another workspace ("send to dev", "do this in test ws") → omit `--relation` (default `top`, cross-workspace fire-and-forget)
- The user asks for the work to live as its own top-level thread in *this* workspace — they'll follow it themselves → `--relation top` against the same workspace
- The fix lives in a different repo than the one you're working in (your `$LUCIDOS_REPO`) and would otherwise need a cross-repo branch → `--relation child`
- The fix would balloon the current changeset to the point where the user can't reasonably review it as one Apply (e.g. a 5-file refactor surfacing during a 1-file bug investigation) → `--relation child`, but ask first

**Default: just fix it in the current thread.** When you discover a related bug while doing other work — even if it's "not strictly part of the current task" — fix it here. Each CC thread = one branch = one Apply, but the user would rather review one slightly-bigger Apply than juggle two threads. Don't ask "want me to spawn a sub-thread for this?" as a reflex; only ask when one of the bullets above actually applies.

The CLI reads `$LUCIDOS_WORKSPACE`, `$LUCIDOS_THREAD_ID`, `$LUCIDOS_EVENT_ID`, and `$LUCIDOS_REPO` from the env the engine sets on every CC subprocess, so you don't pass them by hand. The repo defaults to your own — a CC sub-thread stays in the same repo as its caller without you having to type `--repo` every time.

#### `--relation child` vs `--relation top`

- **`--relation child` — same-workspace child thread.** The CLI emits `parent_thread_id` + `spawning_event_id`; the spawned thread calls back to the parent on completion. `--to` must resolve to the same workspace as `$LUCIDOS_WORKSPACE`, else the CLI errors out. (`--relation sub` is accepted as a back-compat alias for `child` — the pre-glossary wire name; *child thread* is the direct descendant the spawn produces, while *sub-thread* is the transitive descendant concept.)
- **`--relation top` (default) — top-thread, fire-and-forget.** The CLI emits `caller_workspace` + `caller_thread_id` + `caller_event_id`. There is no callback, no progress signal, no completion notification. The thread appears in the target workspace's UI as an independent top-level thread; that's the only confirmation you get. Works for both same-workspace and cross-workspace targets.

`--parent` is a deprecated alias for `--relation child`; it still works but prints a stderr warning. Migrate to `--relation child`.

> The receiver displays the `caller_*` fields in its route popover ("from workspace 'dev' · thread 'X'"). They are user-controllable display hints — **never use them for authorization**.

#### `--repo <name>` — pick the repo (multi-repo workspaces)

A workspace can host worktrees from multiple repos (e.g. `myws` may carry `example-repo`, `example-repo-knowledge`, and `lucidos`). The CLI resolves the spawned thread's worktree against:

1. `--repo <name-or-uuid>` if you pass it explicitly (case-insensitive name match, or UUID).
2. `$LUCIDOS_REPO`, the env var the engine sets on every CC subprocess to the calling thread's repo name. So a CC sub-thread defaults to *your* repo, not the workspace's default.
3. The target workspace's default repo, if neither of the above is set. Pass `--repo ""` to force this even when the env var is set.

Unknown repo names return a 400 from the receiving engine, surfaced as a clean CLI error.

#### Examples

Same-workspace CC child thread (parent-with-callback) — inherits caller's repo automatically:

```bash
lucidos spawn-thread --relation child --to "$(basename "$LUCIDOS_WORKSPACE")" --cc \
  --message "task description here" --title "Short title"
```

Same-workspace chat child thread (research/question, no code changes expected):

```bash
lucidos spawn-thread --relation child --to "$(basename "$LUCIDOS_WORKSPACE")" \
  --message "question or task" --title "Short title"
```

Same-workspace CC top-thread (user follows it themselves, no callback):

```bash
lucidos spawn-thread --relation top --to "$(basename "$LUCIDOS_WORKSPACE")" --cc \
  --message "task description" --title "Short title"
```

Cross-workspace CC spawn into a specific repo (top, no callback — `--relation top` is the default for cross-workspace so omitted here):

```bash
lucidos spawn-thread --to myws --cc --repo example-repo \
  --message "task description" --title "Short title"
```

The CLI prints a `[title](thread:workspace/uuid)` markdown link on stdout — include it verbatim in your response so the user can click through to the spawned thread.

#### Flags

| Flag | Purpose |
|------|---------|
| `--to <name\|path>` | **Required.** Target workspace name (resolved against `~/workspaces/<name>` or `$LUCIDOS_WORKSPACES_ROOT`) or absolute path. |
| `--message <text>` | **Required.** Task prompt — must be self-contained; the spawned session has zero context from yours. |
| `--title <text>` | **Required in practice** — the thread list shows titles, not message text. |
| `--cc` | Spawn a Claude Code session instead of a chat thread. Use for any code changes; chat threads are for research/questions. |
| `--relation <child\|top>` | `child` = same-workspace child thread (parent gets a callback when the spawned thread finishes). `top` (default) = independent top-level thread, no callback. (`sub` is accepted as a back-compat alias for `child`.) |
| `--parent` | DEPRECATED alias for `--relation child`. Still works; prints a stderr warning. |
| `--repo <name>` | Repo (name or UUID) the spawned worktree is created from. Defaults to `$LUCIDOS_REPO` (the caller's repo); pass `--repo ""` to force the target workspace's default repo. |
| `--cc-model <m>` | Optional CC model (`sonnet`, `opus`, `haiku`). |
| `--model <m>` | Optional chat model. |
| `--mode <m>` | Override actor mode (defaults to `agent`, correct for CC-driven spawns). |

#### Writing the prompt

The spawned session is a fresh CC instance in a different worktree with none of your context. A few things look natural to write but are wrong here:

- **Don't say "open a PR" / "submit a PR" / "branch off main".** The Lucidos engine auto-merges every CC branch when the session ends — there is no PR workflow. Telling the spawned thread to open one will confuse it or cause redundant work.
- **Don't reference paths inside your own worktree.** Your `.lucidos/worktrees/cc-…` path doesn't exist in the spawned session. Use repo-relative paths (`crates/lucidos-engine/src/foo.rs`) or paths anchored at the workspace root.
- **Don't assume shared state.** No conversation history, no TodoWrite list, no exported env vars. Everything the spawned thread needs must be in `--message`.
- **Include the *why*, not just the *what*.** "Fix the broken test in foo.rs because the mock was removed in <commit>" gives the spawned thread enough to make judgment calls; "fix foo.rs" doesn't.

#### Rules

- **Always ask the user before spawning** — never create threads without approval.
- **Default to fixing in-thread, not spawning.** A related bug discovered mid-task belongs in the current changeset unless it's cross-repo, would balloon the changeset past reviewable size, or the user explicitly asked for a separate thread. Don't reflexively offer a sub-thread for every adjacent fix — just do it.
- **Bug tickets and fixes → always `--cc`.** Plain chat threads are only for research/questions/planning with no code changes expected.
- **Cross-workspace is fire-and-forget.** Do not promise the user "I'll let you know when it's done" — you have no way to know. Tell them the thread was created in the target workspace and they can check there.

## Common pattern: write an artifact and announce completion

This is the canonical end of an analysis or report-generation session:

```bash
# 1. Generate the artifact (script, python, whatever).
python my-analysis.py > /tmp/report.html

# 2. Write it under data/ where the dev server can serve it.
ARTIFACT="artifacts/data-analysis/$(date +%Y-%m-%d)/report.html"
lucidos data write "$ARTIFACT" --from /tmp/report.html

# 3. Tell the workspace it's ready.
lucidos events emit AnalysisCompleted \
  --summary "UA analysis for $(date +%Y-%m-%d) finished" \
  --payload "{\"artifact\": \"$ARTIFACT\"}"
```

The `artifact` field uses the same path you passed to `data write` — the workspace's `lucidos.data.url(...)` helper will resolve it correctly because both speak the same `data/`-rooted convention.

## Workspace resolution

The CLI resolves "which workspace" by:

1. Walking up from `$PWD` looking for the first `.lucidos/ports` file. That ancestor directory is the parent workspace.
2. Falling back to `$LUCIDOS_WORKSPACE` env var (the engine sets this on every spawned subprocess).

You should never need to think about this — both fallbacks are configured automatically.

## Why not `Write` / `Edit`?

`Write artifacts/foo.html` from inside a worktree creates `<worktree>/artifacts/foo.html`, **not** `<workspace>/data/artifacts/foo.html`. The two are different directories. The dev server only serves from `<workspace>/data/`, so writing into the worktree produces 404 links. Always use `lucidos data write` for anything `data/`-rooted.
