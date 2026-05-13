---
name: Lucidos CLI (`lucidos`)
description: Shell command available on PATH for any subprocess Lucidos spawns (Python, bash, Claude Code) — writes files under `data/`, emits and queries domain events, and calls external APIs through the engine proxy so credentials never appear in script source, args, env vars, or logs. Prefer this over hand-rolling HTTP calls back to the engine and over `curl -H "Authorization: Bearer $CRED_..."`.
---

# `lucidos` CLI

A shell command (`lucidos`) available on the `PATH` of every subprocess Lucidos spawns — Python scripts, bash scripts, Claude Code sessions. Use it whenever a script needs to:

- write files into the workspace's `data/` directory
- emit or query domain events on the workspace's event store
- call an external API that's configured in `data/config/apis.json` (auth header injected by the engine — credential never appears in the script)

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

If you find a script doing `curl -H "Authorization: Bearer $CRED_..."` against an API the workspace already owns a credential for, that's drift — add an `apis.json` entry and switch the script to `lucidos proxy`.

## Workspace resolution

The CLI figures out **which workspace** to talk to in this order:

1. **`$LUCIDOS_WORKSPACE`** environment variable. The engine sets this on every spawned subprocess (Python, bash, CC), so this is the authoritative path for the engine-spawned case.
2. **Walk up from `$PWD`** looking for the first ancestor directory that contains a `.lucidos/ports` file. Fallback for terminal users running the CLI by hand without the env var set.

You should never need to think about this — the env var is configured automatically when the engine spawns the subprocess.

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

For CC sessions it additionally drops a skill file at `<worktree>/.claude/skills/lucidos-cli/SKILL.md` so CC discovers the CLI via its normal skill mechanism. A one-line reminder ("Use the `lucidos` CLI for any data-dir writes or event emits.") in your trigger/app prompt is still good belt-and-braces.

## Implementation

- Source: `crates/lucidos-cli/`
- Shared engine wiring: `crates/lucidos-engine/src/runtime/lucidos_cli.rs` — `lucidos_cli_dir` discovers the binary, `ensure_workspace_bin_symlink` installs the workspace-relative symlink, `workspace_script_env_vars` builds the env var bundle.
- Used by `claude_code.rs` (CC sessions) and `engine/mod.rs::build_script_env_vars` (Python/bash tool calls + scheduled scripts).
- The CLI itself is a thin wrapper — see [`js-sdk.md`](./js-sdk.md) for the equivalent in-browser API used by app UIs.
