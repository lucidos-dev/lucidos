---
name: lucidos-cli
description: Use whenever you need to write a file under the parent workspace's data/ directory (artifacts/, knowhow/, apps/, triggers/) or emit/query a domain event from a Claude Code subprocess running in a Lucidos worktree. Prefer this CLI over Write/Edit for any path that should land at <workspace>/data/... — using Write/Edit puts the file inside the worktree, where the dev server cannot serve it and links 404. Triggers include phrases like "write to artifacts/", "write the report", "save to data/", "emit AnalysisCompleted", "emit a domain event", "query events".
---

# `lucidos` CLI — talking back to the parent workspace

You are running inside a Claude Code subprocess spawned by Lucidos, in an isolated git worktree. The worktree is *not* the workspace — anything you write with `Write`/`Edit` lands inside the worktree and is invisible to the running engine.

The `lucidos` CLI is on your `PATH`. Use it whenever you need to:

- write a file that should appear in the workspace's `data/` directory (artifacts, knowhow, app code, trigger code)
- emit a domain event back to the parent workspace
- query existing events from the parent workspace

## When to use which tool

| Task | Use |
|------|-----|
| Edit source code in this worktree | `Write` / `Edit` |
| Create an artifact the user / app UI will see | `lucidos data write` |
| Tell the workspace something happened | `lucidos events emit` |
| Look up prior events | `lucidos events query` |

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
$ lucidos data path ua-analysis/foo/report.html
/Users/.../workspaces/work/data/artifacts/ua-analysis/foo/report.html

$ lucidos data path knowhow/my-app/notes.md --mkdir
/Users/.../workspaces/work/data/knowhow/my-app/notes.md
```

### `lucidos data write <relative> [--from <path> | -]`

Write content to the resolved absolute path. Creates parent directories. Reads from a local file with `--from <path>`, or from stdin (default, also `--from -`).

```bash
# from a file you generated in /tmp
$ lucidos data write artifacts/ua-analysis/2026-04-20/report.html --from /tmp/report.html

# from stdin
$ echo '{"hello": "world"}' | lucidos data write artifacts/foo.json
```

### `lucidos events emit <EventType> --payload <json> [--summary <str>]`

POST a domain event to the parent workspace's event store. Event types are PascalCase past tense (`AnalysisCompleted`, `DataImported`). The payload **must** include a `summary` string — pass it via `--summary` if you don't want to inline it in the JSON.

```bash
$ lucidos events emit AnalysisCompleted \
    --summary "UA analysis for 2026-04-20 finished" \
    --payload '{"artifact": "artifacts/ua-analysis/2026-04-20/report.html", "rows": 1240}'
```

### `lucidos events query [--type T] [--since iso] [--until iso] [--limit N]`

GET events. Outputs the raw JSON array on stdout — pipe through `jq` if you need to slice it.

```bash
$ lucidos events query --type AnalysisCompleted --limit 5 | jq '.[0]'
```

## Common pattern: write an artifact and announce completion

This is the canonical end of an analysis or report-generation session:

```bash
# 1. Generate the artifact (script, python, whatever).
python my-analysis.py > /tmp/report.html

# 2. Write it under data/ where the dev server can serve it.
ARTIFACT="artifacts/ua-analysis/$(date +%Y-%m-%d)/report.html"
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
