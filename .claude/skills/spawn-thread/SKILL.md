---
name: spawn-thread
description: Use when a task should be handled in a separate CognOS thread — spawns a new thread (chat or CC session) in the same or a different workspace via the CognOS API
---

# Spawn a CognOS Thread

Start a new thread in a CognOS workspace by posting to its API. Use this when:

- A sidequest arises that doesn't belong in the current changeset
- The user asks you to send a task to another workspace (e.g., "do this in test ws")
- You discover something that needs fixing but would distract from the current work
- The user approves a suggested fix for a separate session

## How It Works

CognOS workspaces expose an HTTP API. The port is in `<workspace>/.cognos/ports`.

### 1. Find the port

```bash
# Same workspace (read from environment or ports file)
API_PORT=$(grep API_PORT /path/to/workspace/.cognos/ports | cut -d= -f2)

# Known workspaces
# Personal: ~/workspaces/personal/.cognos/ports
# Dev:     ~/workspaces/dev/.cognos/ports
```

### 2. Start a thread

This skill always sends `sender: "system"` because it's invoked by CC, never by a real user; the validator rejects system spawns missing `parent_thread_id`, so the link to the parent thread is guaranteed. CC exports `$COGNOS_THREAD_ID` into every Bash subprocess — use it as the `parent_thread_id`. Build the JSON with `jq -n --arg parent "$COGNOS_THREAD_ID"` to interpolate it safely.

**CC session** (code changes, file edits):
```bash
jq -n \
  --arg parent "$COGNOS_THREAD_ID" \
  --arg title "Short descriptive title" \
  --arg message "your task description here" \
  '{sender: "system", parent_thread_id: $parent, title: $title, message: $message, use_claude_code: true}' \
| curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" \
  --insecure \
  --data-binary @-
```

**Chat thread** (research, questions, planning — no code changes):
```bash
jq -n \
  --arg parent "$COGNOS_THREAD_ID" \
  --arg title "Short descriptive title" \
  --arg message "your question or task here" \
  '{sender: "system", parent_thread_id: $parent, title: $title, message: $message}' \
| curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" \
  --insecure \
  --data-binary @-
```

One call. CognOS creates a new thread and returns `{"event_id": "..."}`. The thread appears in the target workspace's UI immediately.

### Options

| Field | Type | Purpose |
|-------|------|---------|
| `sender` | string | **Required.** Always `"system"` for this skill — it's invoked by CC, never a real user. |
| `parent_thread_id` | string (UUID) | **Required when `sender == "system"`.** The thread that's spawning this one. Read from `$COGNOS_THREAD_ID`. The validator returns 400 if missing. |
| `spawning_event_id` | string (UUID) | Optional, only allowed when `sender == "system"`. **Omit it** — the Bash subprocess running this skill has no access to its own `ClaudeCodeToolCalled` event id (CC exports only `COGNOS_THREAD_ID`). |
| `title` | string | **Required.** Short descriptive title for the thread (shown in thread list). |
| `message` | string | **Required.** The task prompt. |
| `use_claude_code` | bool | `true` = CC session (code changes). Omit or `false` = chat thread. |
| `cc_model` | string | Optional CC model override (e.g., `"sonnet"`, `"opus"`, `"haiku"`). |
| `model` | string | Optional chat model override. |

### 3. Verify it started

The curl returns `{"event_id": "..."}` on success. The thread will appear in the target workspace's thread list.

## Fire-and-forget — no callback

Spawning a thread via this API is **fire-and-forget from your perspective**. The POST confirms the thread was created — nothing more. You will receive no callback, no status updates, and no completion notification when the spawned thread finishes (unlike `run_thread`, which only callbacks back within the SAME workspace). Do not promise the user you'll "let them know when it's done" or "check back later" — you have no way to know. Tell the user the thread was created and where to find it (target workspace's thread list).

## When to Suggest Spawning

If during your work you notice something that should be done separately, suggest it to the user:

> "I noticed X needs fixing but it's unrelated to our current task. Want me to spawn a separate CC session for it?"

Only spawn after the user confirms. Never spawn threads silently.

## Cross-Workspace

To send work to a different workspace, just read that workspace's ports file:

```bash
# From personal workspace, send task to dev workspace
API_PORT=$(grep API_PORT ~/workspaces/dev/.cognos/ports | cut -d= -f2)
jq -n \
  --arg parent "$COGNOS_THREAD_ID" \
  '{sender: "system", parent_thread_id: $parent, title: "Fix broken test in foo.rs", message: "fix the broken test in foo.rs", use_claude_code: true}' \
| curl -s -X POST "https://localhost:${API_PORT}/api/chat/stream" \
  -H "Content-Type: application/json" \
  --insecure \
  --data-binary @-
```

## Writing the Prompt

The spawned session is a fresh CC instance in a different worktree. It has none of your context. A few things look natural to write but are wrong in CognOS:

- **Don't say "open a PR" / "submit a PR" / "branch off main".** The CognOS engine auto-merges every CC branch back to main when the session ends. There is no PR workflow. Instructing the spawned thread to open one will either confuse it or cause it to do redundant work and fight its own SessionStart guidance. Just describe the task; merging happens automatically.
- **Don't reference paths inside your own worktree.** Your `.cognos/worktrees/cc-…` path doesn't exist in the spawned session. Use repo-relative paths (`crates/cognos-engine/src/foo.rs`) or paths anchored at the workspace root.
- **Don't assume shared in-memory state, env vars, or running processes.** The spawned thread starts cold — it can't see your conversation, your TodoWrite list, your unstaged edits, or variables you exported. Everything it needs to act on must be in the `message` field.
- **Do include the *why*, not just the *what*.** "Fix the broken test in foo.rs because the mock was removed in <commit>" gives the spawned thread enough to make judgment calls. "Fix foo.rs" doesn't.

## Important

- **Always ask before spawning** — never create threads without user approval
- **Always include a title** — the thread list shows titles, not message text
- **Use `--insecure`** — CognOS uses self-signed TLS locally
- **Don't spawn for trivial things** — if it's a one-line fix in a file you're already editing, just do it
