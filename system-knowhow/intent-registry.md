---
name: Intent Registry (source of "Available Intents")
description: How the engine builds the "Available Intents" list and how load_knowhow runs one: filesystem-driven, no cache, no projection. Trigger files in apps/<app>/triggers/ also count as intents, the usual source of "phantom intent" confusion.
---

# Intent Registry

The engine builds the "## Available Intents" section of the chat system prompt fresh from disk on every turn. There is no DB table, projection or cache. The engine walks three filesystem locations and emits one entry per `.md` file.

If an intent ID shows in the prompt but not under `apps/<app>/intents/`, **check the trigger directory next** (see "Three sources" below). This confuses users and audit scripts more than anything else here.

## Three sources

`IntentStore::load_all` in `crates/lucidos-engine/src/core/intents.rs` walks:

| Source | ID format | Notes |
|---|---|---|
| `data/apps/<app>/intents/<name>.md` | `<app>/<name>` | App-scoped intents the user invokes on demand. |
| `data/apps/<app>/triggers/<name>.md` | `<app>/<name>` | App-scoped trigger procedures. **Also exposed as intents**: the LLM can run them through `load_knowhow` even when the trigger isn't firing. |
| `data/triggers/<dir>/*.md` | `<stem>` (filename without `.md`) | Standalone trigger procedures. Same dual role: schedule fires the procedure; the LLM can also invoke it on demand. |

There is no top-level `data/intents/` source. The registry silently ignores files placed there.

`IntentStore::load(id)` searches these roots when `load_knowhow(id)` finds no knowhow doc.
Standalone loading first looks under `data/triggers/<id>/`, then picks a markdown
file there. Listing instead derives the id from each filename's stem.
Keep a standalone trigger's folder and filename stem identical, or a listed
intent may not load under its advertised id.

## Running an intent: `load_knowhow`, in the same turn

An intent runs in the thread that asks for it (ADR 0534). `load_knowhow(<intent id>)` returns the intent body, then the docs its `knowhow:` frontmatter names, then, for an app intent, the app's own knowhow. The turn then does the steps itself, on its own model.

- **A knowhow doc wins an id clash.** `load_knowhow` resolves the id as knowhow first, so a doc at `data/knowhow/<id>.md`, or `data/apps/<app>/knowhow/<name>.md` for an app intent, hides the intent. The intents section of `system-knowhow/workspace-audit` flags it.
- **Use `run_thread` only when the work needs its own context.** A child thread follows its caller's model, effort and provider unless the call pins its own.
- **The retired `execute_intent` tool** answers with a refusal that names the `load_knowhow` call to make instead.

## Why trigger files double as intents

A trigger has two parts: *when to fire* (the schedule, in the `TriggerCreated` event payload) and *what to do* (the procedure, in the `.md` file under `triggers/`). As an intent, the procedure also runs on request ("run the morning dashboard now"). One file, two firing modes.

So an audit that walks only `apps/<app>/intents/` reports trigger-derived IDs as "phantom" intents. They are real, and live next door under `triggers/`.

## An empty registry lists no intents

A workspace with an empty registry gets no "Available Intents" section. The fix is a `.md` file under one of the three sources. The first intent file shows on the next turn, with nothing to restart.

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
