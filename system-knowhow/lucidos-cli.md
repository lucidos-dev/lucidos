---
name: Lucidos CLI (`lucidos`)
description: Shell command available on PATH for any subprocess Lucidos spawns (Python, bash, Claude Code) — writes files under `data/`, emits and queries domain events, lists thread summaries (`lucidos threads list/count`), spawns threads (`lucidos spawn-thread`, including app coding-agent threads via `--folder data/apps/<id>`), lists and applies pending changes (`lucidos changes list` / `lucidos changes apply <id>`), and calls external APIs through the engine proxy so credentials never appear in script source, args, env vars, or logs. Prefer this over hand-rolling HTTP calls back to the engine and over `curl -H "Authorization: Bearer $CRED_..."`.
---

# `lucidos` CLI

A shell command (`lucidos`) available on the `PATH` of every subprocess Lucidos spawns — Python scripts, bash scripts, Claude Code sessions. Use it whenever a script needs to:

- write files into the workspace's `data/` directory
- emit or query domain events on the workspace's event store
- list or count *thread summaries* in the workspace — useful for "is anything still running?" gates in triggers
- spawn a new *thread* — a chat thread, or (`--cc`) a *coding-agent thread* on a repo or an app folder (`--folder data/apps/<id>`) — `lucidos spawn-thread`
- list pending / applied *changes* (`lucidos changes list`) and apply a pending one (the CC-proposed branch waiting on the Apply button) — `lucidos changes apply <id>`
- call an external API that's configured in `data/config/apis.json` (auth header injected by the engine — credential never appears in the script)
- send a push notification to the user without going through an LLM thread

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
- `--source` is a comma-separated list of `chat`, `trigger`, `claude_code`. Omit for all sources.
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

### `lucidos spawn-thread --to <WS> --message <M> [--cc] [--folder <path> | --repo <name>] [--relation child|top] [--title <T>] [--model <M>] [--cc-model <M>]`

Start a new *thread* in another (or this same) workspace — a *chat thread* by default, or a *coding-agent thread* with `--cc`. `--to` names the target workspace (resolved under `$LUCIDOS_WORKSPACES_ROOT`, or an absolute path). Caller provenance (`caller_*` fields) defaults from `$LUCIDOS_WORKSPACE` / `$LUCIDOS_THREAD_ID` / `$LUCIDOS_EVENT_ID`, which the engine sets on every spawned subprocess. Prints a clickable `[title](thread:<ws>/<uuid>)` markdown link on stdout.

`--relation top` (the default) starts an independent thread that does not report back; `--relation child` is a same-workspace parent-with-callback spawn (the calling thread auto-resumes when the child finishes).

**Worktree targeting for `--cc` threads:**

- `--repo <name|uuid>` — create the worktree from a registered *repository*. Defaults from `$LUCIDOS_REPO` (the engine sets it to the calling thread's repo) so a CC sidequest stays in its caller's repo. Pass `--repo ""` to force the target workspace's default repo.
- `--folder <path>` — target an app folder instead, spawning an **app coding-agent thread**. A `data/apps/<id>` value (workspace-relative, resolved on the *target* workspace) creates a sparse-checkout worktree narrowed to that app folder whose *Apply* ff-merges into the workspace's `main` — no `/harden`, no engine restart. This is the same machinery the `run_claude` tool's `folder` argument produces. Only whole app folders are valid; the engine rejects other `data/` subtrees, app subpaths, and non-existent folders.

`--folder` and `--repo` are mutually exclusive, and `--folder` requires `--cc` (the CLI errors before any HTTP round-trip on either). When `--folder` is set the `$LUCIDOS_REPO` default is suppressed — the engine rejects a request that carries both a repo and a folder.

```bash
# Spawn an app coding-agent thread to work on an app in this workspace.
$ lucidos spawn-thread --to personal --cc --relation top \
    --folder data/apps/momentum-autoresearch \
    --title "Autoresearch session" \
    --message "Run one research session per data/apps/momentum-autoresearch/knowhow."
[Autoresearch session](thread:personal/2f1c…)
```

### `lucidos notify --title <T> --message <M> [--app-id <APP>] [--tap <T>] [--thread-id <UUID>] [--event-id <UUID>]`

Send a push notification via the parent workspace. Persists to the inbox AND fans out as a web push to subscribed devices — identical to a `send_notification` LLM tool call, but callable directly from any subprocess (Python script, bash script, scheduled `script:`-typed trigger) without going through an LLM thread.

```bash
$ lucidos notify --title "Nettbank pappa" --message "Sjekk nettbanken til pappa (Alf Tiller)"
{"success":true,"notification_id":"5b1e..."}
```

Both `--title` and `--message` are required and must be non-empty (the engine returns 400 on empty values).

`--app-id <id>` is optional and stamps the notification's deep-link target. **Only set it when tapping the notification should open that app to act on it** — most reminders / nudges / summaries shouldn't deep-link, even when their trigger lives inside an app dir for organizational reasons. Same rule the `send_notification` LLM tool follows.

#### Deep-linking back to the originating event

For event-driven triggers ("Claude is asking", "credential needed", …) the right behaviour is for the push tap to scroll straight to the specific event card the user needs to act on. Three flags wire this up:

- **`--tap <modal|none|navigate>`** — which kind of tap. `modal` (default) opens the inbox modal. `none` is the passive variant — no destination; the row marks itself read on in-app toast display or OS push tap (which just launches the PWA). Use for purely informational pushes that need no follow-up ("Backup complete", "Sync finished"). `navigate` deep-links to the target inferred from the other flags: `--thread-id` → navigate to that thread (scrolling and pulsing `--event-id` when set); `--app-id` → navigate to that app. When both `--thread-id` and `--app-id` are present, thread wins (the more common CTA shape — "answer this question").
- **`--thread-id <UUID>`** — the originating thread. With `--tap navigate`, the tap deep-links straight to this thread instead of the inbox modal. Even without `--tap`, this stamps the notification so the modal's "Open thread" button resolves.
- **`--event-id <UUID>`** — a specific event id inside `--thread-id` to scroll to and briefly pulse when the tap lands. Ignored when `--thread-id` is absent.

```bash
# Deep-link the push to the exact UserQuestionAsked card on tap.
lucidos notify \
  --title "Claude is asking" \
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

Apply a pending *change* (a CC-proposed branch that's waiting on the Apply button). Wraps `POST /api/v1/changes/<id>/apply` and echoes the engine's typed `ApplyChangeResult` JSON to stdout. Get the id from `lucidos changes list` (`.pending[].id`).

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

If a script genuinely needs to call the HTTP endpoint directly (test harness, external tool that can't shell out to the CLI), forward both headers explicitly:

```bash
curl -k -X POST \
  -H "x-lucidos-agent-origin-token: $LUCIDOS_AGENT_ORIGIN_TOKEN" \
  -H "x-lucidos-source-thread-id: $LUCIDOS_THREAD_ID" \
  "https://localhost:$LUCIDOS_API_PORT/api/v1/changes/$CID/apply"
```

The engine listens on `https://` with a self-signed cert in dev (`-k` / `_create_unverified_context()` accepts it; the CLI already does). The token env var is process-local secret state set by the engine on every spawned subprocess; the thread id is set when the subprocess has a spawning thread. See `docs/apply-change-api.md` for the response shape and the full apply workflow.

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
| Apply a CC-proposed change from a script | `lucidos changes apply <id>` (never hand-roll the HTTP call — actor stamps as "You") |

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
