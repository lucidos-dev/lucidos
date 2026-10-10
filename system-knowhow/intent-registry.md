---
name: Intent Registry (source of "Available Intents")
description: How the engine builds the "Available Intents" list it exposes via execute_intent: filesystem-driven, no cache, no projection. Trigger files in apps/<app>/triggers/ also count as intents, the usual source of "phantom intent" confusion.
---

# Intent Registry

The engine builds the "## Available Intents" section of the chat system prompt fresh from disk on every turn. There is no DB table, projection or cache. The engine walks three filesystem locations and emits one entry per `.md` file.

If an intent ID shows in the prompt but not under `apps/<app>/intents/`, **check the trigger directory next** (see "Three sources" below). This confuses users and audit scripts more than anything else here.

## Three sources

`IntentStore::load_all` in `crates/lucidos-engine/src/core/intents.rs` walks:

| Source | ID format | Notes |
|---|---|---|
| `data/apps/<app>/intents/<name>.md` | `<app>/<name>` | App-scoped intents the user invokes on demand. |
| `data/apps/<app>/triggers/<name>.md` | `<app>/<name>` | App-scoped trigger procedures. **Also exposed as intents**: the LLM can invoke them via `execute_intent` even when the trigger isn't firing. |
| `data/triggers/<dir>/*.md` | `<stem>` (filename without `.md`) | Standalone trigger procedures. Same dual role: schedule fires the procedure; the LLM can also invoke it on demand. |

There is no top-level `data/intents/` source. The registry silently ignores files placed there.

`IntentStore::load(id)` searches the same paths when `execute_intent(id)` runs. So the loader and the prompt agree by construction: anything listed is loadable, and anything loadable is listed.

## Why trigger files double as intents

A trigger has two parts: *when to fire* (the schedule, in the `TriggerCreated` event payload) and *what to do* (the procedure, in the `.md` file under `triggers/`). As an intent, the procedure also runs on request ("run the morning dashboard now"). One file, two firing modes.

So an audit that walks only `apps/<app>/intents/` reports trigger-derived IDs as "phantom" intents. They are real, and live next door under `triggers/`.

## An empty registry means no `execute_intent` tool at all

The tool is **capability-gated** (ADR 0088). A workspace with an empty registry gets no `execute_intent`, because no id exists to pass it. So "the tool is missing" and "the registry found nothing" are one fact. The fix for both is a `.md` file under one of the three sources.

The gate depends on the workspace, never the thread, so every thread in a workspace agrees about it. It also opens by itself: the first intent file makes the tool appear on the next turn, with nothing to restart.

## Invalidation

None needed. `read_turn_capabilities` rescans the three sources every turn, and `build_chat_system_prompt` lists that same snapshot. Add, remove or rename a `.md` file and the next turn sees it.

## How to enumerate the live registry from a shell

```bash
DATA=~/workspaces/<ws>/data
ls $DATA/apps/*/intents/*.md  2>/dev/null
ls $DATA/apps/*/triggers/*.md 2>/dev/null
ls $DATA/triggers/*/*.md      2>/dev/null
```

The live system prompt's "## Available Intents" section is the ground truth. If it disagrees with the shell list, the engine wins: the loader rejected a file (for example, for invalid frontmatter).
