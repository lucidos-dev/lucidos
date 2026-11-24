---
name: Running Python
description: How to use run_python, run_python_background and bash_output: picking the right one, the venv layout, importing from app scripts, and the antipatterns (sleep-poll, sys.path thrashing). Load BEFORE writing run_python code if you haven't already this thread.
---

# Running Python

How the chat agent runs Python in a workspace, and which mistakes silently burn context.

A failed command that names a protected path gets diagnostic-log guidance.
Successful commands do not get that failure guidance.

The shared background task outcome lives in `crates/lucidos-core/src/core/shell.rs`.
Its description is used by the tool result and the completion event.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/running-python/pick-the-right-tool` | Pick the right tool; The drain pattern: never poll with `time.sleep` (You do not have to sit through it: ending the turn is a valid wait; A restart ends the task, and you will be told so; Never estimate elapsed time: read it; Oversized windows keep the tail); Deciding whether a background task succeeded; The command guard (dangerous commands are gated) |
| `system-knowhow/running-python/the-per-workspace-venv` | The per-workspace venv; Importing from `data/apps/<x>/scripts/`; Anti-patterns that burn context; Errors; Diagnostic evidence |
