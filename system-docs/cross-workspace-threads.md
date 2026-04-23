---
name: Cross-Workspace CognOS Threads
description: How to start threads (chat or Claude Code sessions) in other CognOS workspaces from this workspace
---

# Cross-Workspace CognOS Threads

Start threads (chat or Claude Code sessions) in other CognOS workspaces from this workspace.

## When to use this

- When the user says "send to dev", "run in dev", "dev workspace" or similar → **ALWAYS** use cross-workspace API
- Keywords indicating another workspace: "send to", "run in", "push to", "task to" + workspace name
- **NEVER** use `run_thread` or `run_claude` for tasks targeting another workspace — they run locally
- Code tasks → `"use_claude_code": true`, research/chat → omit or `false`

## Fire-and-forget — no callback, no status

Cross-workspace POSTs are **completely fire-and-forget from the sender's perspective**. Unlike `run_thread` (same workspace, parent gets a callback when the child finishes), a POST to another workspace's `/api/chat/stream`:

- Returns immediately with `{"event_id": "..."}` — that confirms the thread was created, nothing more
- Sends NO callback when the task completes
- Sends NO progress updates, NO status changes, NO completion notification
- The receiving workspace cannot reach back into yours

**Do NOT promise the user things like "I'll let you know when it's done" or "I'll check back in a few minutes."** You will not be notified. You have no way to know whether the task succeeded, failed, or is still running. After the POST, your job is done — tell the user the thread was created in the target workspace and that they can check there for progress/results.

## 1. Find running instances

Each CognOS instance stores its port in `<workspace>/.cognos/ports`.

### Discover workspaces dynamically

Run the discovery script to list all CognOS workspaces and their status:

```bash
bash system-docs/scripts/list-workspaces.sh
```

Output shows workspace name, status (RUNNING/STOPPED), and ports path.

### Read the port for a specific workspace

```bash
API_PORT=$(grep API_PORT ~/workspaces/dev/.cognos/ports | cut -d= -f2)
```

`/api/health` returns `version`, `workspace` and `uptime`.

## 2. Start a thread

### Chat thread (research, questions, planning)

```bash
curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" \
  --insecure \
  -d '{"mode": "human", "message": "task description here"}'
```

### Claude Code session (code changes, fixes)

```bash
curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" \
  --insecure \
  -d '{"mode": "human", "message": "task description here", "use_claude_code": true}'
```

Returns `{"event_id": "..."}` on success. The thread appears immediately in the target workspace's UI.

### Parameters

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | **Required.** Task prompt. Must be self-contained — the receiver has no context from the sender thread. |
| `mode` | string | **Required.** Who originated this message: `"human"` (a person typed it), `"agent"` (an LLM-driven thread is spawning this), or `"engine"` (engine-internal). For an LLM-spawned cross-workspace task, use `"agent"` + `parent_thread_id` + `spawning_event_id`. The legacy `"sender"` field with values `"user"` / `"system"` still deserializes (`user` → `human`, `system` → `agent`). |
| `parent_thread_id` | uuid | **Required when `mode` is `"agent"` or `"engine"`.** The thread in the calling workspace that is spawning this new thread. MUST be omitted when `mode` is `"human"`. |
| `spawning_event_id` | uuid | The event in the parent thread that triggered this spawn (e.g. the `ToolCalled` event for a `run_thread` invocation). Allowed only when `mode` is `"agent"` or `"engine"`. |
| `title` | string | **Recommended.** Thread title shown in the target workspace's UI. Caller should always set this to a short, descriptive name for the task. If omitted, the system generates one from the first message. |
| `use_claude_code` | bool | `true` = CC session (code). Omit or `false` = chat thread. |
| `cc_model` | string | Optional CC model (e.g. `"sonnet"`, `"opus"`, `"haiku"`). |
| `model` | string | Optional chat model. |

## 3. Origin headers

The receiving engine stamps every inbound message with a `MessageOrigin` so the target workspace's UI can show "from workspace 'dev' · thread 'X'" in the thread route popover. For cross-workspace calls, the source is carried in HTTP headers, **not** in the request body:

| Header | Purpose |
|--------|---------|
| `X-Cognos-Workspace` | Name of the calling workspace (e.g. `dev`, `personal`). Triggers `MessageOrigin::Workspace`. |
| `X-Cognos-Thread-Id` | Optional. UUID of the source thread in the calling workspace. |
| `X-Cognos-Event-Id` | Optional. UUID of the source event (e.g. the tool-call event that initiated the POST). |
| `X-Cognos-Mode` | Optional. Upstream actor mode (`human` / `agent` / `engine`). Defaults to `human` if absent. |

Cross-workspace POSTs are typically issued from a CC subprocess via `curl` (the engine has no built-in cross-workspace POST helper today — `run_thread` is same-workspace only). When you hand-roll a cross-workspace `curl`, set these headers if you want the receiver's route popover to show your origin. They are optional — omit them and the receiver falls back to `MessageOrigin::Api { user_agent }`.

These headers are **display hints only**. They are user-controllable — any HTTP client can set them — and MUST NOT be relied on for authorization.

## 4. Example: Send task to dev workspace

```bash
API_PORT=$(grep API_PORT ~/workspaces/dev/.cognos/ports | cut -d= -f2)
curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" \
  --insecure \
  -d '{"mode": "human", "message": "Investigate the bug in capture_app", "title": "Fix capture_app bug", "use_claude_code": true}'
```

## Rules

- **Bug tickets and fixes → always Claude Code** — use `"use_claude_code": true` for any bug report, bugfix, or code-related ticket sent to dev. Only use plain chat threads for research/questions/planning with no code changes expected.
- **Always ask the user before spawning** — never create threads without approval
- **Use `--insecure`** — CognOS uses self-signed TLS locally
- **Write self-contained prompts** — spawned session has zero context from your session
- **Don't spawn for trivial things** — do it yourself if it's a simple change
- **Never hardcode ports** — always read from the ports file or run discovery
