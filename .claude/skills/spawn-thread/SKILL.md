---
name: spawn-thread
description: Use when a task should be handled in a separate Lucidos thread — spawns a new thread (chat or CC session) in the same or a different workspace via the `lucidos send-thread` CLI
---

# Spawn a Lucidos Thread

Start a new thread in a Lucidos workspace by invoking the `lucidos send-thread` CLI. Use this when:

- A sidequest arises that doesn't belong in the current changeset
- The user asks you to send a task to another workspace (e.g., "do this in test ws")
- You discover something that needs fixing but would distract from the current work
- The user approves a suggested fix for a separate session

## How It Works

The engine sets these env vars on every Claude Code subprocess — the CLI reads them automatically, so you don't pass them by hand:

- `$LUCIDOS_WORKSPACE` — absolute path to the parent workspace; basename becomes `caller_workspace`
- `$LUCIDOS_THREAD_ID` — current thread UUID; becomes `caller_thread_id` (or `parent_thread_id` with `--parent`)
- `$LUCIDOS_EVENT_ID` — the tool-call event UUID; becomes `caller_event_id` (or `spawning_event_id` with `--parent`)

### `--parent` vs no flag

- **Same workspace, parent-with-callback** (most common from CC): pass `--parent`. The CLI emits `parent_thread_id` + `spawning_event_id` in the body, and the spawned thread will call back to the parent on completion. `--to` must resolve to the same workspace as `$LUCIDOS_WORKSPACE` (else error).
- **Cross-workspace, fire-and-forget**: omit `--parent`. The CLI emits `caller_workspace` + `caller_thread_id` + `caller_event_id`. There is no callback — see "Fire-and-forget" below.

### Examples

**Same-workspace CC spawn** (parent-with-callback — the typical sidequest):

```bash
lucidos send-thread --parent --to "$(basename "$LUCIDOS_WORKSPACE")" --cc \
  --message "task description" --title "Short title"
```

**Same-workspace chat thread spawn** (research/question, no code changes):

```bash
lucidos send-thread --parent --to "$(basename "$LUCIDOS_WORKSPACE")" \
  --message "question or task" --title "Short title"
```

**Cross-workspace CC spawn** (no callback):

```bash
lucidos send-thread --to dev --cc \
  --message "task description" --title "Short title"
```

The CLI prints `{"event_id": "..."}` on success. The thread appears immediately in the target workspace's UI.

### Flags

| Flag | Purpose |
|------|---------|
| `--to <name\|path>` | **Required.** Target workspace name (resolved against `~/workspaces/<name>` or `$LUCIDOS_WORKSPACES_ROOT`) or absolute path. |
| `--message <text>` | **Required.** Task prompt. |
| `--title <text>` | **Required in practice** — the thread list shows titles, not message text. |
| `--cc` | Spawn a Claude Code session instead of a chat thread. |
| `--parent` | Same-workspace parent-with-callback semantics (see above). |
| `--cc-model <m>` | Optional CC model (`sonnet`, `opus`, `haiku`). |
| `--model <m>` | Optional chat model. |
| `--mode <m>` | Override actor mode (defaults to `agent`, which is correct for CC-driven spawns). |

## Fire-and-forget — no callback

A cross-workspace POST (no `--parent`) is **fire-and-forget from your perspective**. The CLI confirms the thread was created — nothing more. You will receive no callback, no status updates, and no completion notification. Do not promise the user you'll "let them know when it's done" or "check back later" — you have no way to know. Tell the user the thread was created and where to find it (target workspace's thread list).

`--parent` (same-workspace) does deliver a callback to the parent thread when the child finishes — that's the only mode where a follow-up signal exists.

## When to Suggest Spawning

If during your work you notice something that should be done separately, suggest it to the user:

> "I noticed X needs fixing but it's unrelated to our current task. Want me to spawn a separate CC session for it?"

Only spawn after the user confirms. Never spawn threads silently.

## Writing the Prompt

The spawned session is a fresh CC instance in a different worktree. It has none of your context. A few things look natural to write but are wrong in Lucidos:

- **Don't say "open a PR" / "submit a PR" / "branch off main".** The Lucidos engine auto-merges every CC branch back to main when the session ends. There is no PR workflow. Instructing the spawned thread to open one will either confuse it or cause it to do redundant work and fight its own SessionStart guidance. Just describe the task; merging happens automatically.
- **Don't reference paths inside your own worktree.** Your `.lucidos/worktrees/cc-…` path doesn't exist in the spawned session. Use repo-relative paths (`crates/lucidos-engine/src/foo.rs`) or paths anchored at the workspace root.
- **Don't assume shared in-memory state, env vars, or running processes.** The spawned thread starts cold — it can't see your conversation, your TodoWrite list, your unstaged edits, or variables you exported. Everything it needs to act on must be in the `--message` argument.
- **Do include the *why*, not just the *what*.** "Fix the broken test in foo.rs because the mock was removed in <commit>" gives the spawned thread enough to make judgment calls. "Fix foo.rs" doesn't.

## Important

- **Always ask before spawning** — never create threads without user approval
- **Always include `--title`** — the thread list shows titles, not message text
- **Don't spawn for trivial things** — if it's a one-line fix in a file you're already editing, just do it
