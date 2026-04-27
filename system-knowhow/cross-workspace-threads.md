---
name: Cross-Workspace Lucidos Threads
description: Start threads (chat or Claude Code sessions) in other Lucidos workspaces from this workspace
---

# Cross-Workspace Lucidos Threads

Start threads (chat or Claude Code sessions) in other Lucidos workspaces from this workspace.

## When to use this

- When the user says "send to dev", "run in dev", "dev workspace" or similar → **ALWAYS** use cross-workspace API
- Keywords indicating another workspace: "send to", "run in", "push to", "task to" + workspace name
- **NEVER** use `run_thread` or `run_claude` for tasks targeting another workspace — they run locally
- Code tasks → `--cc` (chat threads otherwise)

## Fire-and-forget — no callback, no status

Cross-workspace POSTs are **completely fire-and-forget from the sender's perspective**. Unlike `run_thread` (same workspace, parent gets a callback when the child finishes), a POST to another workspace's `/api/chat/stream`:

- Returns immediately with `{"event_id": "..."}` — that confirms the thread was created, nothing more
- Sends NO callback when the task completes
- Sends NO progress updates, NO status changes, NO completion notification
- The receiving workspace cannot reach back into yours

**Do NOT promise the user things like "I'll let you know when it's done" or "I'll check back in a few minutes."** You will not be notified. You have no way to know whether the task succeeded, failed, or is still running. After the POST, your job is done — tell the user the thread was created in the target workspace and that they can check there for progress/results.

## 1. Use the CLI (recommended)

Inside a Claude Code subprocess (where the engine sets `$LUCIDOS_WORKSPACE`, `$LUCIDOS_THREAD_ID`, and `$LUCIDOS_EVENT_ID`), use the `lucidos send-thread` subcommand. It defaults the `caller_*` body fields from those env vars so you only need `--to`, `--message`, and `--title`.

### Chat thread (research, questions, planning)

```bash
lucidos send-thread --to dev --message "task description here" --title "Short title"
```

### Claude Code session (code changes, fixes)

```bash
lucidos send-thread --to dev --cc --message "task description here" --title "Short title"
```

The CLI sets `mode=agent`, `caller_workspace=$LUCIDOS_WORKSPACE` (basename), `caller_thread_id=$LUCIDOS_THREAD_ID`, `caller_event_id=$LUCIDOS_EVENT_ID` on every POST. Override `mode` with `--mode` if you need `human` or `engine`.

### Discover workspaces

Each Lucidos instance stores its port in `<workspace>/.lucidos/ports`. To list every workspace and its running status:

```bash
bash system-knowhow/scripts/list-workspaces.sh
```

The CLI resolves `--to dev` against `~/workspaces/dev` (or `$LUCIDOS_WORKSPACES_ROOT/dev` when set). Pass an absolute path to `--to` to bypass the lookup.

## 2. Raw curl (fallback)

When the CLI isn't available (e.g., from a script that can't shell out to `lucidos`), POST directly. Read the API port from the target workspace's ports file:

```bash
API_PORT=$(grep API_PORT ~/workspaces/dev/.lucidos/ports | cut -d= -f2)
curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" --insecure \
  -d '{
    "mode": "agent",
    "caller_workspace": "personal",
    "caller_thread_id": "<uuid>",
    "caller_event_id": "<uuid>",
    "message": "task description here",
    "title": "Short title",
    "use_claude_code": true
  }'
```

### Body fields

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | **Required.** Task prompt — must be self-contained. |
| `mode` | string | **Required.** `"human"` (a person typed it), `"agent"` (LLM-driven), or `"engine"` (engine-internal). |
| `caller_workspace` | string | Cross-workspace origin: name of the calling workspace. Triggers `MessageOrigin::Workspace` on the receiver. |
| `caller_thread_id` | uuid | Optional. UUID of the source thread in the calling workspace. Allowed only with `caller_workspace`. |
| `caller_event_id` | uuid | Optional. UUID of the source event (e.g. tool-call event). Allowed only with `caller_workspace`. |
| `parent_thread_id` | uuid | **Same-workspace only** — receiver returns 400 if combined with `caller_workspace`. Required when `mode` is `"agent"`/`"engine"` AND no `caller_workspace` is set. |
| `spawning_event_id` | uuid | **Same-workspace only.** Allowed only with `parent_thread_id`. |
| `title` | string | Recommended. Thread title shown in the target workspace's UI. |
| `use_claude_code` | bool | `true` = CC session. Omit or `false` = chat thread. |
| `cc_model` | string | Optional CC model (`"sonnet"`, `"opus"`, `"haiku"`). |
| `model` | string | Optional chat model. |

## 3. Origin

The receiver constructs `MessageOrigin::Workspace { workspace, thread_id, event_id, mode, .. }` from the body's `caller_*` fields. The route popover in the target workspace's UI surfaces it as "from workspace 'dev' · thread 'X'".

> Caller fields are user-controllable display hints — never use them for authorization.

## Rules

- **Bug tickets and fixes → always Claude Code** — pass `--cc` for any bug report, bugfix, or code-related ticket sent to dev. Only use plain chat threads for research/questions/planning with no code changes expected.
- **Always ask the user before spawning** — never create threads without approval
- **Use `--insecure` with raw curl** — Lucidos uses self-signed TLS locally (the CLI handles this for you)
- **Write self-contained prompts** — spawned session has zero context from your session
- **Don't spawn for trivial things** — do it yourself if it's a simple change
- **Never hardcode ports** — let the CLI find them, or read from the ports file
