---
name: Lucidos CLI (`lucidos`)
description: Shell command available on PATH for any subprocess Lucidos spawns (Python, bash, Claude Code) — writes files under `data/`, emits and queries domain events. Prefer this over hand-rolling HTTP calls back to the engine.
---

# `lucidos` CLI

A shell command (`lucidos`) available on the `PATH` of every subprocess Lucidos spawns — Python scripts, bash scripts, Claude Code sessions. Use it whenever a script needs to:

- write files into the workspace's `data/` directory
- emit or query domain events on the workspace's event store

The CLI is a thin Rust wrapper around the engine's HTTP API and filesystem conventions — for app UI usage see the JS [`lucidos.data.*`](./js-sdk.md) reference. Scripts should always prefer the CLI over hand-rolling HTTP calls back to the engine.

## When to use this

- **Python scripts** (`apps/<name>/scripts/*.py`, `triggers/<name>/scripts/*.py`, `knowhow/*/scripts/*.py`) — invoke via `subprocess.run(['lucidos', 'data', 'write', ...])`. The CLI is on PATH and `LUCIDOS_WORKSPACE` is set automatically.
- **Bash scripts** (same locations, `*.sh`) — call `lucidos` directly. PATH is set up before the script runs.
- **CC subprocesses** — running in a worktree under `<workspace>/.lucidos/worktrees/<id>/`, where `Write`/`Edit` lands in the worktree (not the workspace), so dev-server links 404. Use `lucidos data write` instead.

For app UIs (in the browser) keep using the JS SDK (`lucidos.data.*`, `lucidos.events.*`). The CLI is for shell / subprocess contexts only.

## Subcommands

### `lucidos data path <relative> [--mkdir]`

Print the absolute filesystem path that `<relative>` resolves to inside the parent workspace's `data/` directory.

Path normalization matches `normalizeDataPath()` in the artifacts UI: paths starting with `artifacts/`, `knowhow/`, `apps/`, or `triggers/` are kept; anything else is prefixed with `artifacts/`.

```bash
$ lucidos data path artifacts/ua-analysis/foo/report.html
/Users/.../workspaces/work/data/artifacts/ua-analysis/foo/report.html

$ lucidos data path report.html
/Users/.../workspaces/work/data/artifacts/report.html

$ lucidos data path knowhow/myapp/notes.md --mkdir
/Users/.../workspaces/work/data/knowhow/myapp/notes.md
# (parent dir created)
```

`--mkdir` creates the parent directory chain. Useful when piping the result to another tool.

### `lucidos data write <relative> [--from <local-path> | -]`

Write content to the resolved absolute path. Creates parent dirs automatically.

```bash
# from a local file you generated elsewhere
$ lucidos data write artifacts/ua-analysis/2026-04-20/report.html --from /tmp/report.html

# from stdin (default; also `--from -`)
$ echo '{"hello": "world"}' | lucidos data write artifacts/foo.json
```

The resolved absolute path is printed on **stderr** so it can be captured separately from any future structured stdout.

### `lucidos events emit <EventType> --payload <json> [--summary <str>]`

POST a domain event to the parent workspace's event store.

- `event_type` is PascalCase past tense (`AnalysisCompleted`, `DataImported`).
- `payload` must be a JSON **object** containing a `summary` string. If you pass `--summary`, it overrides / injects `summary` into the payload before sending.

```bash
$ lucidos events emit AnalysisCompleted \
    --summary "UA analysis for 2026-04-20 complete" \
    --payload '{"artifact": "artifacts/ua-analysis/2026-04-20/report.html", "rows": 1240}'
```

The CLI prints the server's JSON response on stdout (`{"success": true, "event_id": "..."}`).

### `lucidos events query [--type T] [--since iso] [--until iso] [--limit N]`

GET events from the parent workspace's event store. Outputs the raw JSON array on stdout.

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

## Workspace resolution

The CLI figures out **which workspace** to talk to in this order:

1. **Walk up from `$PWD`** looking for the first ancestor directory that contains a `.lucidos/ports` file. That ancestor is the workspace.
2. **Fall back to `$LUCIDOS_WORKSPACE`** environment variable. The engine sets this on every spawned subprocess (Python, bash, CC), so the fallback always works in the engine-spawned case.

The walk-up step naturally finds the right workspace from inside a worktree:

```
<workspace>/.lucidos/worktrees/<id>/foo/bar     <- $PWD
<workspace>/.lucidos/worktrees/<id>/foo
<workspace>/.lucidos/worktrees/<id>
<workspace>/.lucidos/worktrees
<workspace>/.lucidos                           <- no .lucidos/ports here
<workspace>                                    <- .lucidos/ports found, stop
```

You should never need to think about this — both fallbacks are configured automatically when the engine spawns the subprocess.

## Common patterns

### Write an artifact and emit a completion event

The canonical end of an analysis or report-generation session. This is the pattern the UA analysis app's prompt should use instead of trying to write into the worktree:

```bash
ARTIFACT="artifacts/ua-analysis/$(date +%Y-%m-%d)/report.html"

# 1. Write the artifact under data/.
lucidos data write "$ARTIFACT" --from /tmp/report.html

# 2. Tell the workspace it's ready.
lucidos events emit AnalysisCompleted \
  --summary "UA analysis for $(date +%Y-%m-%d) finished" \
  --payload "{\"artifact\": \"$ARTIFACT\"}"
```

Both calls speak the same `data/`-rooted path convention, so an SSE listener that does `lucidos.data.url(payload.artifact)` in the frontend will resolve the link correctly.

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

For CC sessions it additionally drops a skill file at `<worktree>/.claude/skills/lucidos-cli/SKILL.md` so CC discovers the CLI via its normal skill mechanism. A one-line reminder ("Use the `lucidos` CLI for any data-dir writes or event emits.") in your trigger/app prompt is still good belt-and-braces.

## Implementation

- Source: `crates/lucidos-cli/`
- Shared engine wiring: `crates/lucidos-engine/src/runtime/lucidos_cli.rs` — `lucidos_cli_dir` discovers the binary, `ensure_workspace_bin_symlink` installs the workspace-relative symlink, `workspace_script_env_vars` builds the env var bundle.
- Used by `claude_code.rs` (CC sessions) and `engine/mod.rs::build_script_env_vars` (Python/bash tool calls + scheduled scripts).
- The CLI itself is a thin wrapper — see [`js-sdk.md`](./js-sdk.md) for the equivalent in-browser API used by app UIs.
