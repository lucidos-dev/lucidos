---
name: Intent Registry — Source of "Available Intents"
description: How the engine builds the "Available Intents" list shown in the system prompt and exposed via execute_intent. Filesystem-driven, no cache, no projection. Trigger files in apps/<app>/triggers/ also count as intents — common source of "phantom intent" confusion.
---

# Intent Registry

The "## Available Intents" section of the engine system prompt is built fresh from disk every time a chat thread starts (or any time the system prompt is reconstructed). There is no DB table, no projection, no cache to invalidate — the engine walks three filesystem locations and emits one entry per `.md` file it finds.

If an intent ID appears in the prompt but you can't find it under `apps/<app>/intents/`, **check the trigger directory next** (see "Three sources" below). It is the most common source of confusion for both users and audit scripts.

## Three sources

`crates/lucidos-engine/src/core/intents.rs` — `IntentStore::load_all` walks:

| Source | ID format | Notes |
|---|---|---|
| `data/apps/<app>/intents/<name>.md` | `<app>/<name>` | App-scoped intents the user invokes on demand. |
| `data/apps/<app>/triggers/<name>.md` | `<app>/<name>` | App-scoped trigger procedures. **Also exposed as intents** — the LLM can invoke them via `execute_intent` even when the trigger isn't firing. |
| `data/triggers/<dir>/*.md` | `<stem>` (filename without `.md`) | Standalone trigger procedures. Same dual role: schedule fires the procedure; the LLM can also invoke it on demand. |

Notably absent: there is no top-level `data/intents/` source. Files placed there are silently ignored by the registry.

The same paths are searched in reverse by `IntentStore::load(id)` when `execute_intent(id)` runs, so the loader and the prompt are in lockstep by construction — anything listed in the prompt is loadable, and anything loadable is listed.

## Why trigger files double as intents

A trigger has two dimensions: *when to fire* (schedule, lives in the `TriggerCreated` event payload) and *what to do when it fires* (procedure, lives in the `.md` file under `triggers/`). Exposing the procedure as an intent lets the user manually invoke it ("run the morning dashboard now") without duplicating the recipe — one file, two firing modes.

This is also why an audit that walks only `apps/<app>/intents/` will report trigger-derived IDs as "phantom" intents. They are real; they just live next door under `triggers/`.

## Invalidation

None needed. The list is recomputed every time `process_chat_message` builds the system prompt. Add, remove, or rename a `.md` file under any of the three sources and the next thread sees the change. There is nothing to purge, regenerate, or clear.

## How to enumerate the live registry from a shell

```bash
DATA=~/workspaces/<ws>/data
ls $DATA/apps/*/intents/*.md  2>/dev/null
ls $DATA/apps/*/triggers/*.md 2>/dev/null
ls $DATA/triggers/*/*.md      2>/dev/null
```

The active engine system prompt's "## Available Intents" section is the ground truth — if the shell list and the prompt section disagree, the engine wins (something in the loader rejected a file, e.g. invalid frontmatter).
